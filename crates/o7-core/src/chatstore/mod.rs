//! The chat store: one writer thread owns the profile's [`Store`], and every write and read goes
//! through its FIFO, so a read sees every write queued before it.
//!
//! - **FIFO, group commit.** Typed [`Op`]s queue in one FIFO bounded at 1,024 ops and 32 MiB (one
//!   larger op passes alone). The writer drains up to 256 ops or 4 MiB into one `BEGIN IMMEDIATE`
//!   transaction; an op never spans transactions, and a read ends the batch so it sees everything
//!   before it. Session actors reserve room first ([`ChatStore::reserve`]): that is the backpressure
//!   that reaches the agents when saving stalls.
//! - **Disk full.** The failed batch is kept and retried every 5 s while the writer stops taking ops;
//!   reads are served from saved data through a second channel meanwhile, with what is unsaved
//!   marked. Nothing is lost unless o7 quits while paused.
//! - **Busy, poison, damage.** Busy past the timeout retries with backoff, then pauses; any other
//!   error re-runs the batch one op at a time and sends an op that fails again to the dead-letter file;
//!   damage is recorded for the next open's repair and later ops go to the dead-letter file.
//! - **Blobs** are files under `<profile>/attachments/<sha256>`, written before the op whose row
//!   names them.
//! - **Shutdown** commits what is queued within the deadline and closes the connection, which folds
//!   the WAL back so the next launch sees a clean shutdown.
//!
//! In normal operation every call goes through the FIFO; one that gets no answer within 5 s fails
//! with [`StoreError::Busy`] ([`StoreError::DiskFull`] while a full disk pauses saving). While
//! saving is paused, `chats`, `read_blob`, `read_saved_page` and `load_chat` (for a chat with nothing
//! unsaved and no running turn) are served from saved data at once.

pub(crate) mod agent_catalogs;
pub(crate) mod blobs;
pub(crate) mod compact;
pub(crate) mod composer;
pub(crate) mod conversation;
pub(crate) mod coordinator;
mod export;
pub(crate) mod files;
pub(crate) mod folder_resolution;
pub(crate) mod history;
pub(crate) mod image_refs;
pub(crate) mod line_edits;
pub(crate) mod memory;
pub(crate) mod organization;
pub(crate) mod recover;
pub(crate) mod rows;
pub(crate) mod search;
pub(crate) mod setup_attempt;
pub(crate) mod steering;
#[cfg(test)]
pub(crate) mod test_support;
pub(crate) mod workspace_activity;
pub(crate) mod workspace_removal;
pub(crate) mod worktree_setup;
mod writer;

use std::collections::HashSet;
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use o7_protocol::session::{BlobText, HistoryPage, PauseReason, StorageStatus};
use o7_protocol::{Autonomy, Millis, StopReason};
use serde::Serialize;
use tokio::runtime::Handle;
use tokio::sync::{oneshot, watch};

use crate::error::CoreError;
use crate::owner::ProfileOwner;
use crate::profile::Profile;
use crate::store::Store;

use writer::{Permits, Request, Shared, Work, Writer};
#[cfg(test)]
pub(crate) use writer::{SavedRead, TestWork};
pub(crate) use writer::{WaitError, WriterConfig, await_reply, await_reply_while, op_bytes};

/// A call that gets no answer within this fails.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// The longest a call waits for an answer while the writer is at work it knows to be long
/// (startup recovery, building an index): it is not busy then, only not ready yet.
const SETTLING_LIMIT: Duration = Duration::from_secs(120);

/// How long one call for line edits goes on counting turns that have not been counted yet before it
/// answers with what it has.
const LINE_EDITS_COUNTING: Duration = Duration::from_secs(3);

/// A shutdown reports success only if the writer also closed the connection within this much more.
const CLOSE_MARGIN: Duration = Duration::from_secs(2);

/// Dropping the store without a shutdown still commits what is queued, within this.
const DROP_DEADLINE: Duration = Duration::from_secs(5);

/// The profile's chats, owned by one writer thread.
pub struct ChatStore {
    shared: Arc<Shared>,
    rt: Handle,
    db_path: PathBuf,
    attachment_dir: PathBuf,
    recovered: watch::Receiver<Option<RecoveryReport>>,
}

impl fmt::Debug for ChatStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatStore")
            .field("db_path", &self.db_path)
            .field("status", &*self.shared.status.borrow())
            .finish_non_exhaustive()
    }
}

impl ChatStore {
    pub async fn shared_mcp(
        &self,
        command: crate::mcp::Command,
    ) -> Result<crate::mcp::Reply, StoreError> {
        let (reply, answer) = oneshot::channel();
        if !command.writes() {
            return self
                .request(Request::SharedMcp { command, reply }, answer)
                .await?;
        }
        if self.is_paused() {
            return Err(self.timed_out());
        }
        let permits = tokio::time::timeout(REQUEST_TIMEOUT, self.permits(command.bytes()))
            .await
            .map_err(|_| self.timed_out())?
            .ok_or(StoreError::Closed)?;
        self.shared.push(
            Work::Request(Request::SharedMcp { command, reply }),
            Some(permits),
        );
        answer.await.map_err(|_| StoreError::Closed)?
    }
    pub(crate) async fn memory(
        &self,
        command: memory::Command,
    ) -> Result<memory::Reply, StoreError> {
        let (reply, answer) = oneshot::channel();
        if command.read_only() {
            return self
                .request(Request::Memory { command, reply }, answer)
                .await?;
        }
        if self.is_paused() {
            return Err(self.timed_out());
        }
        let permits = tokio::time::timeout(REQUEST_TIMEOUT, self.permits(command.bytes()))
            .await
            .map_err(|_| self.timed_out())?
            .ok_or(StoreError::Closed)?;
        self.shared.push(
            Work::Request(Request::Memory { command, reply }),
            Some(permits),
        );
        answer.await.map_err(|_| StoreError::Closed)?
    }

    /// Coordinated projects: reads answer at once, writes wait their turn on the writer.
    pub(crate) async fn coordinator(
        &self,
        command: coordinator::Command,
    ) -> Result<coordinator::Reply, StoreError> {
        let (reply, answer) = oneshot::channel();
        if command.read_only() {
            return self
                .request(Request::Coordinator { command, reply }, answer)
                .await?;
        }
        if self.is_paused() {
            return Err(self.timed_out());
        }
        let permits = tokio::time::timeout(REQUEST_TIMEOUT, self.permits(command.bytes()))
            .await
            .map_err(|_| self.timed_out())?
            .ok_or(StoreError::Closed)?;
        self.shared.push(
            Work::Request(Request::Coordinator { command, reply }),
            Some(permits),
        );
        answer.await.map_err(|_| StoreError::Closed)?
    }

    /// Every committed change to a project's coordination, from the project page or an agent.
    pub fn coordinator_changes(
        &self,
    ) -> tokio::sync::broadcast::Receiver<o7_protocol::coordinator::CoordinatorChanged> {
        self.shared.coordinator_changes.subscribe()
    }

    pub(crate) async fn setup_attempt(
        &self,
        command: setup_attempt::Command,
    ) -> Result<Option<o7_protocol::worktree_setup::WorktreeSetupAttempt>, StoreError> {
        if self.is_paused() {
            return Err(self.timed_out());
        }
        let (reply, answer) = oneshot::channel();
        let permits = tokio::time::timeout(REQUEST_TIMEOUT, self.permits(2048))
            .await
            .map_err(|_| self.timed_out())?
            .ok_or(StoreError::Closed)?;
        self.shared.push(
            Work::Request(Request::SetupAttempt { command, reply }),
            Some(permits),
        );
        answer.await.map_err(|_| StoreError::Closed)?
    }
    pub(crate) async fn read_setup_attempt(
        &self,
        chat_id: &str,
    ) -> Result<Option<o7_protocol::worktree_setup::WorktreeSetupAttempt>, StoreError> {
        let (reply, answer) = oneshot::channel();
        self.request(
            Request::ReadSetupAttempt {
                chat_id: chat_id.into(),
                reply,
            },
            answer,
        )
        .await?
    }
    pub(crate) async fn folder_resolution(
        &self,
        command: folder_resolution::Command,
    ) -> Result<folder_resolution::Reply, StoreError> {
        let (reply, answer) = oneshot::channel();
        if !command.writes() {
            return self
                .request(Request::FolderResolution { command, reply }, answer)
                .await?;
        }
        if self.is_paused() {
            return Err(self.timed_out());
        }
        let permits = tokio::time::timeout(REQUEST_TIMEOUT, self.permits(command.bytes()))
            .await
            .map_err(|_| self.timed_out())?
            .ok_or(StoreError::Closed)?;
        self.shared.push(
            Work::Request(Request::FolderResolution { command, reply }),
            Some(permits),
        );
        answer.await.map_err(|_| StoreError::Closed)?
    }
    /// The agents' model catalogs, saved as one device-only setting of this profile.
    pub(crate) async fn agent_catalogs(
        &self,
        command: agent_catalogs::Command,
    ) -> Result<Option<String>, StoreError> {
        let (reply, answer) = oneshot::channel();
        if !command.writes() {
            return self
                .request(Request::AgentCatalogs { command, reply }, answer)
                .await?;
        }
        if self.is_paused() {
            return Err(self.timed_out());
        }
        let permits = tokio::time::timeout(REQUEST_TIMEOUT, self.permits(command.bytes()))
            .await
            .map_err(|_| self.timed_out())?
            .ok_or(StoreError::Closed)?;
        self.shared.push(
            Work::Request(Request::AgentCatalogs { command, reply }),
            Some(permits),
        );
        answer.await.map_err(|_| StoreError::Closed)?
    }
    pub(crate) fn workspace_changes(&self) -> watch::Receiver<u64> {
        self.shared.workspace_activity.subscribe()
    }
    /// Every committed change to a project's memory, history or proposals, whether the editor or
    /// an agent made it, naming an agent's direct edit. A lagging receiver should re-read
    /// everything.
    pub fn memory_changes(
        &self,
    ) -> tokio::sync::broadcast::Receiver<o7_protocol::memory::ProjectMemoryChanged> {
        self.shared.memory_changes.subscribe()
    }
    #[cfg(test)]
    pub(crate) fn remove_workspace_before_begin(&self, chat: &str) {
        self.shared.workspace_activity.remove_before_begin(chat);
    }
    pub(crate) fn workspace_available(&self, chat: &str) -> bool {
        self.shared.workspace_activity.available(chat)
    }
    pub(crate) fn begin_workspace_turn(&self, chat: &str) -> Option<workspace_activity::Turn> {
        self.shared.workspace_activity.begin(chat)
    }
    pub(crate) async fn workspace_removal(
        &self,
        command: workspace_removal::Command,
    ) -> Result<workspace_removal::Reply, StoreError> {
        let (reply, answer) = oneshot::channel();
        if !command.writes() {
            return self
                .request(Request::WorkspaceRemoval { command, reply }, answer)
                .await?;
        }
        if self.is_paused() {
            return Err(self.timed_out());
        }
        let permits = tokio::time::timeout(REQUEST_TIMEOUT, self.permits(command.bytes()))
            .await
            .map_err(|_| self.timed_out())?
            .ok_or(StoreError::Closed)?;
        self.shared.push(
            Work::Request(Request::WorkspaceRemoval { command, reply }),
            Some(permits),
        );
        answer.await.map_err(|_| StoreError::Closed)?
    }
    /// Every agent's line edits since `since` ([`line_edits`]), after counting the turns that ended
    /// since the last call, newest first. Each pass reads on a connection of its own on a blocking
    /// thread, never holding the writer, and saves what it counted through the FIFO; passes go on
    /// for at most [`LINE_EDITS_COUNTING`], and turns still waiting make the answer incomplete, so
    /// the Dashboard shows what is counted and asks again.
    pub(crate) async fn line_edits(
        &self,
        since: Millis,
    ) -> Result<o7_protocol::activity::LineEdits, StoreError> {
        if self.is_paused() {
            return Err(self.timed_out());
        }
        let started = Instant::now();
        let mut complete = false;
        while started.elapsed() < LINE_EDITS_COUNTING {
            let db = self.db_path.clone();
            let attachments = files::Attachments::new(self.attachment_dir.clone());
            let (counted, more) = self
                .rt
                .spawn_blocking(move || line_edits::count_pass(&db, &attachments))
                .await
                .map_err(|_| StoreError::Other("Counting line edits stopped.".into()))?
                .map_err(|error| {
                    StoreError::Other(format!("Couldn't count line edits: {error}"))
                })?;
            if !counted.is_empty() {
                let (reply, answer) = oneshot::channel();
                self.request(Request::SaveLineEdits { counted, reply }, answer)
                    .await??;
            }
            if !more {
                complete = true;
                break;
            }
        }
        let (reply, answer) = oneshot::channel();
        let buckets = self
            .request(Request::ReadLineEdits { since, reply }, answer)
            .await??;
        Ok(o7_protocol::activity::LineEdits { buckets, complete })
    }
    pub(crate) async fn search(
        &self,
        query: String,
        include_archived: bool,
    ) -> Result<o7_protocol::search::SearchResults, StoreError> {
        let (reply, answer) = oneshot::channel();
        if self.is_paused() {
            return self
                .saved(
                    writer::SavedRead::Search {
                        query,
                        include_archived,
                        reply,
                    },
                    answer,
                )
                .await?;
        }
        self.request(
            Request::Search {
                query,
                include_archived,
                reply,
            },
            answer,
        )
        .await?
    }
    pub(crate) async fn worktree_setup(
        &self,
        command: worktree_setup::Command,
    ) -> Result<o7_protocol::worktree_setup::WorktreeSetup, StoreError> {
        let (reply, answer) = oneshot::channel();
        if command.save.is_none() {
            return self
                .request(Request::WorktreeSetup { command, reply }, answer)
                .await?;
        }
        if self.is_paused() {
            return Err(self.timed_out());
        }
        let permits = tokio::time::timeout(REQUEST_TIMEOUT, self.permits(command.bytes()))
            .await
            .map_err(|_| self.timed_out())?
            .ok_or(StoreError::Closed)?;
        self.shared.push(
            Work::Request(Request::WorktreeSetup { command, reply }),
            Some(permits),
        );
        answer.await.map_err(|_| StoreError::Closed)?
    }
    pub(crate) async fn steering(&self, command: steering::Command) -> Result<(), StoreError> {
        if self.is_paused() {
            return Err(self.timed_out());
        }
        let (reply, answer) = oneshot::channel();
        let permits = tokio::time::timeout(REQUEST_TIMEOUT, self.permits(command.bytes()))
            .await
            .map_err(|_| self.timed_out())?
            .ok_or(StoreError::Closed)?;
        self.shared.push(
            Work::Request(Request::Steering { command, reply }),
            Some(permits),
        );
        answer.await.map_err(|_| StoreError::Closed)?
    }
    pub(crate) async fn conversation(
        &self,
        command: conversation::ConversationCommand,
    ) -> Result<conversation::ConversationReply, StoreError> {
        let (reply, answer) = oneshot::channel();
        if !command.writes() {
            return self
                .request(Request::Conversation { command, reply }, answer)
                .await?;
        }
        if self.is_paused() {
            return Err(self.timed_out());
        }
        let permits = tokio::time::timeout(REQUEST_TIMEOUT, self.permits(command.bytes()))
            .await
            .map_err(|_| self.timed_out())?
            .ok_or(StoreError::Closed)?;
        self.shared.push(
            Work::Request(Request::Conversation { command, reply }),
            Some(permits),
        );
        answer.await.map_err(|_| StoreError::Closed)?
    }
    pub(crate) async fn composer(
        &self,
        command: composer::ComposerCommand,
    ) -> Result<Vec<composer::StoredAttachment>, StoreError> {
        match self.composer_command(command).await? {
            composer::ComposerReply::Attachments(items) => Ok(items),
            composer::ComposerReply::Draft(_) | composer::ComposerReply::ImageParts(_) => {
                Err(StoreError::Other("Unexpected draft response.".into()))
            }
        }
    }

    pub(crate) async fn composer_command(
        &self,
        command: composer::ComposerCommand,
    ) -> Result<composer::ComposerReply, StoreError> {
        if self.is_paused() {
            return Err(self.timed_out());
        }
        let permits = tokio::time::timeout(REQUEST_TIMEOUT, self.permits(command.bytes()))
            .await
            .map_err(|_| self.timed_out())?
            .ok_or(StoreError::Closed)?;
        let (reply, answer) = oneshot::channel();
        self.shared.push(
            Work::Request(Request::Composer { command, reply }),
            Some(permits),
        );
        answer.await.map_err(|_| StoreError::Closed)?
    }
    pub(crate) async fn checkpoint_context(
        &self,
        chat_id: &str,
        turn_id: Option<String>,
    ) -> Result<(ChatRecord, Option<String>), StoreError> {
        let (reply, answer) = oneshot::channel();
        if self.is_paused() {
            return self
                .saved(
                    writer::SavedRead::CheckpointContext {
                        chat_id: chat_id.into(),
                        turn_id,
                        reply,
                    },
                    answer,
                )
                .await?;
        }
        self.request(
            Request::CheckpointContext {
                chat_id: chat_id.into(),
                turn_id,
                reply,
            },
            answer,
        )
        .await?
    }

    pub(crate) async fn organize(
        &self,
        command: organization::OrganizationCommand,
    ) -> Result<organization::Organization, StoreError> {
        let (reply, answer) = oneshot::channel();
        if command.is_read() {
            if self.is_paused()
                && let organization::OrganizationCommand::Snapshot(filter) = command
            {
                return self
                    .saved(writer::SavedRead::Organization { filter, reply }, answer)
                    .await?;
            }
            return self
                .request(Request::Organization { command, reply }, answer)
                .await?;
        }
        let permits = tokio::time::timeout(REQUEST_TIMEOUT, self.permits(command.bytes()))
            .await
            .map_err(|_| self.timed_out())?
            .ok_or(StoreError::Closed)?;
        self.shared.push(
            Work::Request(Request::Organization { command, reply }),
            Some(permits),
        );
        // Mutations acknowledge the actual transaction, never a timeout followed by a late move.
        answer.await.map_err(|_| StoreError::Closed)?
    }
    /// Moves the opened `Store` and the owner lock into the writer thread; global recovery is its
    /// first op.
    pub fn start(
        store: Store,
        owner: ProfileOwner,
        profile: &Profile,
        device_id: String,
        rt: Handle,
    ) -> Result<Arc<ChatStore>, CoreError> {
        ChatStore::start_with(store, owner, profile, device_id, rt, WriterConfig::STANDARD)
    }

    /// [`start`](Self::start) with other limits and timers (tests).
    pub(crate) fn start_with(
        store: Store,
        owner: ProfileOwner,
        profile: &Profile,
        device_id: String,
        rt: Handle,
        config: WriterConfig,
    ) -> Result<Arc<ChatStore>, CoreError> {
        let files = files::Files::new(profile);
        let shared = Arc::new(Shared::new(config));
        let (recovered, recovered_rx) = watch::channel(None);
        let writer = Writer::new(
            Arc::clone(&shared),
            store,
            owner,
            device_id,
            files,
            recovered,
        );
        let finished = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("aura-chat-writer".into())
            .spawn(move || {
                // Every op runs under its own unwind guard; this one only keeps a panic elsewhere in
                // the writer from leaving callers waiting on a thread that is gone.
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| writer.run())).is_err()
                {
                    tracing::error!("the chat store's writer thread panicked; saving stopped");
                    finished.abandon();
                }
            })
            .map_err(CoreError::io(profile.db_path()))?;
        let store = Arc::new(ChatStore {
            shared,
            rt,
            db_path: profile.db_path(),
            attachment_dir: profile.attachments_dir(),
            recovered: recovered_rx,
        });
        if config.leftovers {
            store.rt.spawn(ChatStore::leftovers(Arc::downgrade(&store)));
        }
        Ok(store)
    }

    /// What startup leaves for afterwards, as ordinary queued work that reads and live chats go
    /// between: every ended turn that is not compacted yet is compacted, now and each time the
    /// writer says one may wait, and a while in, the indexes recovery reads through are built when
    /// they are missing. A turn too large to merge is read on a connection of its own, off the
    /// writer, and only its text is saved through the FIFO. Ends when the store closes.
    async fn leftovers(store: Weak<ChatStore>) {
        let Some(this) = store.upgrade() else {
            return;
        };
        this.recovered().await;
        let shared = Arc::clone(&this.shared);
        drop(this);
        // The indexes wait a while: the window is asking for its first lists right now. A thread of
        // its own keeps the time, not the runtime's clock: a timer here would be one more thing a
        // test on paused time has to account for.
        let delay = shared.config.index_delay;
        if delay.is_zero() {
            shared.set_index_due();
        } else {
            let due = Arc::clone(&shared);
            let spawned = std::thread::Builder::new()
                .name("aura-chat-indexes".into())
                .spawn(move || {
                    std::thread::sleep(delay);
                    due.set_index_due();
                });
            if spawned.is_err() {
                shared.set_index_due();
            }
        }
        let mut indexed = false;
        // Each turn is tried once: one that cannot be compacted must not be read again and again.
        let mut tried: HashSet<String> = HashSet::new();
        loop {
            let Some(this) = store.upgrade() else {
                return;
            };
            match this.compact_leftovers(&mut tried).await {
                Ok(()) => {}
                Err(StoreError::Closed) => return,
                Err(error) => {
                    tracing::warn!(%error, "could not compact the turns left from earlier; a later pass tries again");
                }
            }
            if !indexed && shared.index_due() {
                indexed = true;
                let (reply, answer) = oneshot::channel();
                match this
                    .request(Request::EnsureIndexes { reply }, answer)
                    .await
                    .and_then(|built| built)
                {
                    Ok(_) => {}
                    Err(StoreError::Closed) => return,
                    Err(error) => {
                        tracing::warn!(%error, "could not build the indexes recovery reads through; the next start tries again");
                    }
                }
            }
            drop(this);
            // Woken for a turn, when the indexes are due, or because the store closed.
            shared.leftovers.notified().await;
        }
    }

    /// One pass of [`leftovers`](Self::leftovers) over the turns not in `tried`.
    async fn compact_leftovers(&self, tried: &mut HashSet<String>) -> Result<(), StoreError> {
        if self.is_paused() {
            return Ok(());
        }
        let (reply, answer) = oneshot::channel();
        let waiting = self.request(Request::Leftovers { reply }, answer).await??;
        let limits = self.shared.config.compact;
        for turn in waiting {
            if !tried.insert(turn.turn_id.clone()) {
                continue;
            }
            if turn.bytes <= limits.merge_limit {
                let permit = self.reserve(256).await.map_err(|_| StoreError::Closed)?;
                permit.send(Op::CompactTurn {
                    chat_id: turn.chat_id,
                    turn_id: turn.turn_id,
                });
                continue;
            }
            let db = self.db_path.clone();
            let turn_id = turn.turn_id.clone();
            let rows = self
                .rt
                .spawn_blocking(move || compact::collect_text_at(&db, &turn_id, &limits))
                .await
                .map_err(|_| StoreError::Other("Reading a turn's text stopped.".into()))?
                .map_err(|error| {
                    StoreError::Other(format!("Couldn't read a turn's text: {error}"))
                })?;
            let (reply, answer) = oneshot::channel();
            self.request(
                Request::SaveTurnText {
                    chat_id: turn.chat_id,
                    turn_id: turn.turn_id,
                    rows,
                    reply,
                },
                answer,
            )
            .await??;
        }
        Ok(())
    }

    /// Whether saving runs, is paused (with how much is unsaved), or failed.
    pub fn status(&self) -> watch::Receiver<StorageStatus> {
        self.shared.status.subscribe()
    }

    pub fn is_paused(&self) -> bool {
        self.shared.is_paused()
    }

    pub(crate) fn attachment_dir(&self) -> &std::path::Path {
        &self.attachment_dir
    }

    /// How long startup's work on the database took, as far as it is known yet.
    pub fn startup_timings(&self) -> StartupTimings {
        let (recovery_ms, indexes_ms) = self.shared.startup_timings();
        StartupTimings {
            recovery_ms,
            indexes_ms,
        }
    }

    /// What startup recovery did, once it finished.
    pub async fn recovered(&self) -> RecoveryReport {
        let mut recovered = self.recovered.clone();
        match recovered.wait_for(Option::is_some).await {
            Ok(report) => report.clone().unwrap_or_default(),
            Err(_) => RecoveryReport::default(),
        }
    }

    /// The backpressure point for actors: room for one op of about `bytes` in the FIFO.
    pub async fn reserve(&self, bytes: usize) -> Result<OpPermit, StoreClosed> {
        let permits = self.permits(bytes).await.ok_or(StoreClosed)?;
        Ok(OpPermit {
            shared: Arc::clone(&self.shared),
            permits,
        })
    }

    async fn permits(&self, bytes: usize) -> Option<Permits> {
        let count = Arc::clone(&self.shared.count).acquire_owned().await.ok()?;
        let wanted = bytes.clamp(1, self.shared.config.fifo_bytes);
        let bytes = Arc::clone(&self.shared.bytes)
            .acquire_many_owned(u32::try_from(wanted).unwrap_or(u32::MAX))
            .await
            .ok()?;
        Some(Permits::new(count, bytes))
    }

    pub async fn create_chat(&self, new: NewChat) -> Result<ChatRecord, StoreError> {
        let (reply, answer) = oneshot::channel();
        self.request(Request::CreateChat { new, reply }, answer)
            .await?
    }

    pub async fn chats(&self) -> Result<Vec<ChatRecord>, StoreError> {
        let (reply, answer) = oneshot::channel();
        if self.is_paused() {
            return self
                .saved(writer::SavedRead::Chats { reply }, answer)
                .await?;
        }
        self.request(Request::Chats { reply }, answer).await?
    }

    /// Per-chat recovery of turns left `running` (an actor that died), then everything an actor
    /// needs. Every queued row comes back held: none was queued by the actor that loads it.
    pub async fn load_chat(&self, chat_id: &str) -> Result<ChatLoad, StoreError> {
        self.load_chat_inner(chat_id)
            .await?
            .ok_or_else(|| StoreError::Other(format!("There is no chat with id {chat_id}.")))
    }

    /// [`load_chat`](Self::load_chat), with `None` for a chat that does not exist.
    pub(crate) async fn load_chat_inner(
        &self,
        chat_id: &str,
    ) -> Result<Option<ChatLoad>, StoreError> {
        let (reply, answer) = oneshot::channel();
        let chat_id = chat_id.to_owned();
        if self.is_paused() {
            return self
                .saved(writer::SavedRead::LoadChat { chat_id, reply }, answer)
                .await?;
        }
        self.request(Request::LoadChat { chat_id, reply }, answer)
            .await?
    }

    /// A bounded Markdown snapshot of saved events, serialized by the profile writer.
    pub async fn export_markdown(&self, chat_id: &str) -> Result<String, StoreError> {
        let (reply, answer) = oneshot::channel();
        self.request(
            Request::ExportMarkdown {
                chat_id: chat_id.to_owned(),
                reply,
            },
            answer,
        )
        .await?
    }

    /// One page of a blob's text, ending on a character boundary.
    pub async fn read_blob(
        &self,
        sha256: &str,
        offset: u64,
        len: u32,
    ) -> Result<BlobText, StoreError> {
        if !blobs::is_sha256(sha256) {
            return Err(StoreError::BlobMissing(blobs::BLOB_GONE.to_owned()));
        }
        let (reply, answer) = oneshot::channel();
        let sha256 = sha256.to_owned();
        if self.is_paused() {
            return self
                .saved(
                    writer::SavedRead::ReadBlob {
                        sha256,
                        offset,
                        len,
                        reply,
                    },
                    answer,
                )
                .await?;
        }
        self.request(
            Request::ReadBlob {
                sha256,
                offset,
                len,
                reply,
            },
            answer,
        )
        .await?
    }

    /// While paused: the page from saved rows only, with `unsaved_from` set.
    pub async fn read_saved_page(
        &self,
        chat_id: &str,
        before_seq: u32,
        max_bytes: usize,
    ) -> Result<HistoryPage, StoreError> {
        let (reply, answer) = oneshot::channel();
        self.saved(
            writer::SavedRead::Page {
                chat_id: chat_id.to_owned(),
                before_seq,
                max_bytes,
                reply,
            },
            answer,
        )
        .await?
    }

    /// Dismisses the database repair notice (see [`Store::acknowledge_recovery`]).
    pub async fn acknowledge_recovery(&self, quarantined: String) -> Result<bool, CoreError> {
        let (reply, answer) = oneshot::channel();
        self.request(Request::Acknowledge { quarantined, reply }, answer)
            .await
            .map_err(|error| CoreError::Io {
                path: self.db_path.clone(),
                source: io::Error::other(error.to_string()),
            })?
    }

    /// Commits what is queued, then closes the connection. Returns whether everything was saved
    /// within `deadline`.
    ///
    /// Use [`Self::flush`] first when a reversible transition must verify pending writes before
    /// beginning irreversible shutdown.
    pub async fn shutdown(&self, deadline: Duration) -> bool {
        let (done, finished) = std::sync::mpsc::channel();
        self.shared
            .request_shutdown(Instant::now() + deadline, done);
        let limit = deadline + CLOSE_MARGIN;
        self.rt
            .spawn_blocking(move || writer::wait_finished(&finished, limit))
            .await
            .unwrap_or(false)
    }

    /// Wait for every earlier FIFO write while keeping the writer open. A pause, failure or
    /// timeout refuses the transition; cancellation never closes or drops queued work.
    pub async fn flush(&self, deadline: Duration) -> Result<(), StoreError> {
        let (reply, answer) = oneshot::channel();
        tokio::time::timeout(deadline, self.request(Request::Flush { reply }, answer))
            .await
            .map_err(|_| self.timed_out())??
    }

    /// The same from any thread, with or without a runtime (`RunEvent::Exit`); never panics.
    pub fn shutdown_blocking(&self, deadline: Duration) -> bool {
        let (done, finished) = std::sync::mpsc::channel();
        self.shared
            .request_shutdown(Instant::now() + deadline, done);
        writer::wait_finished(&finished, deadline + CLOSE_MARGIN)
    }

    /// Sends work through the FIFO and waits for its answer, both within [`REQUEST_TIMEOUT`].
    async fn request<T: Send + 'static>(
        &self,
        request: Request,
        answer: oneshot::Receiver<T>,
    ) -> Result<T, StoreError> {
        let started = tokio::time::Instant::now();
        let permits = match tokio::time::timeout(REQUEST_TIMEOUT, self.permits(256)).await {
            Ok(Some(permits)) => permits,
            Ok(None) => return Err(StoreError::Closed),
            Err(_) => return Err(self.timed_out()),
        };
        self.shared.push(Work::Request(request), Some(permits));
        let left = REQUEST_TIMEOUT.saturating_sub(started.elapsed());
        self.wait(answer, left).await
    }

    /// Sends a read to the second channel and waits for its answer.
    async fn saved<T: Send + 'static>(
        &self,
        read: writer::SavedRead,
        answer: oneshot::Receiver<T>,
    ) -> Result<T, StoreError> {
        self.shared.push_saved(read);
        self.wait(answer, REQUEST_TIMEOUT).await
    }

    async fn wait<T: Send + 'static>(
        &self,
        answer: oneshot::Receiver<T>,
        limit: Duration,
    ) -> Result<T, StoreError> {
        // While the writer recovers or builds an index, the answer is late, not lost.
        let settling = || self.shared.is_settling();
        match await_reply_while(&self.rt, answer, limit, SETTLING_LIMIT.max(limit), settling).await
        {
            Ok(value) => Ok(value),
            Err(WaitError::Gone) => Err(StoreError::Closed),
            Err(WaitError::TimedOut) => Err(self.timed_out()),
        }
    }

    /// What a call that got no answer in time fails with.
    fn timed_out(&self) -> StoreError {
        match self.shared.pause_reason() {
            Some(PauseReason::DiskFull) => StoreError::DiskFull,
            Some(PauseReason::Busy) | None => StoreError::Busy,
        }
    }

    /// Tests: queues test work without room in the FIFO.
    #[cfg(test)]
    pub(crate) fn test_work(&self, work: TestWork) {
        self.shared.push(Work::Test(work), None);
    }

    /// Tests: runs `run` on the writer's store through the second channel (even while paused).
    #[cfg(test)]
    pub(crate) fn with_store(&self, run: impl FnOnce(&mut Store) + Send + 'static) {
        self.shared.push_saved(SavedRead::WithStore(Box::new(run)));
    }

    /// Waits until every op queued before it is committed (or its batch failed).
    #[cfg(test)]
    pub(crate) async fn barrier(&self) -> bool {
        let (reply, answer) = oneshot::channel();
        let Ok(permit) = self.reserve(1).await else {
            return false;
        };
        permit.send(Op::Barrier(reply));
        self.wait(answer, Duration::from_secs(60)).await.is_ok()
    }
}

impl Drop for ChatStore {
    fn drop(&mut self) {
        // Commits what is queued and closes, without waiting here.
        let (done, _finished) = std::sync::mpsc::channel();
        self.shared
            .request_shutdown(Instant::now() + DROP_DEADLINE, done);
    }
}

/// Room for one op in the FIFO.
pub struct OpPermit {
    shared: Arc<Shared>,
    permits: Permits,
}

impl OpPermit {
    pub fn send(self, op: Op) {
        self.shared.push(Work::Op(op), Some(self.permits));
    }
}

impl fmt::Debug for OpPermit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpPermit").finish_non_exhaustive()
    }
}

/// One unit of work for the writer thread. An op is atomic: it never spans transactions.
#[derive(Debug)]
pub enum Op {
    InsertAgentSession(AgentSessionRow),
    /// Keeps (or with `None`, forgets) the session the chat's terminal last used.
    SaveTerminalSession {
        chat_id: String,
        record: Option<TerminalSessionRecord>,
    },
    UpdateAgentSession {
        id: String,
        patch: AgentSessionPatch,
    },
    /// Commits this native identity in its own transaction and acknowledges the actual result.
    /// Unlike a barrier, a failed or dead-lettered earlier write cannot report success here.
    SaveNativeIdentity {
        id: String,
        native_id: String,
        updated_at: Millis,
        reply: oneshot::Sender<Result<(), StoreError>>,
    },
    /// Inserts the turn, appends `rows` (TurnStarted, the user's text, ...) and deletes the queued
    /// prompt `turn.prompt_id`, in one transaction: a crash keeps either the queued prompt or the
    /// started turn.
    BeginTurn {
        turn: TurnRow,
        rows: Vec<EventRow>,
    },
    AppendEvents {
        chat_id: String,
        rows: Vec<EventRow>,
    },
    EndTurn(TurnEnd),
    CompactTurn {
        chat_id: String,
        turn_id: String,
    },
    PutBlob {
        sha256: String,
        mime: String,
        bytes: Arc<[u8]>,
    },
    SetTitle {
        chat_id: String,
        title: String,
        source: TitleSource,
    },
    /// The line edits a terminal's turn made, from the agent's own session record, as
    /// `(when, lines)` ([`line_edits::record`]): its stored events hold no tool calls.
    RecordLineEdits {
        chat_id: String,
        turn_id: String,
        edits: Vec<(Millis, u64)>,
    },
    TouchChat {
        chat_id: String,
        at: Millis,
    },
    QueuePut(QueuedRow),
    QueueRemove {
        id: String,
    },
    QueueHoldAll {
        chat_id: String,
    },
    QueueReleaseAll {
        chat_id: String,
    },
    /// History read through the actor's outbox, so a page follows every write the actor queued
    /// before it. The latest page is `before_seq = next_seq`.
    ReadPage {
        chat_id: String,
        before_seq: u32,
        max_bytes: usize,
        reply: oneshot::Sender<Result<HistoryPage, StoreError>>,
    },
    Barrier(oneshot::Sender<()>),
}

/// How a folder path is kept in `folder_locations.path`, and bound where that column is compared:
/// the plain spelling of a Windows verbatim path whenever it names the same folder
/// ([`crate::fsutil::plain_path`]; migration 14 rewrote the rows saved before).
pub(crate) fn stored_path(path: &std::path::Path) -> String {
    crate::fsutil::plain_path(path)
        .to_string_lossy()
        .into_owned()
}

/// [`stored_path`] for a path the caller holds as text.
pub(crate) fn stored_path_text(path: &str) -> String {
    crate::fsutil::plain_windows_path(path).unwrap_or_else(|| path.to_owned())
}

impl Op {
    /// The op's name for logs.
    pub(crate) fn name(&self) -> &'static str {
        writer::op_name(self)
    }

    /// A read or a barrier: dropping it loses no data (its waiter learns the store closed).
    pub(crate) fn is_read(&self) -> bool {
        matches!(self, Op::ReadPage { .. } | Op::Barrier(_))
    }
}

/// A chat to create in a folder on this device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NewChat {
    pub agent_id: String,
    pub agent_host: o7_protocol::agents::AgentHost,
    pub folder: PathBuf,
    pub device_id: String,
    pub now: Millis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChatRecord {
    pub id: String,
    pub folder_id: Option<String>,
    pub title: Option<String>,
    pub title_source: Option<TitleSource>,
    pub agent_id: String,
    pub agent_host: o7_protocol::agents::AgentHost,
    pub folder: PathBuf,
    pub created_at: Millis,
    pub updated_at: Millis,
}

/// Everything a chat actor starts from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChatLoad {
    pub worktree_setup: Option<o7_protocol::worktree_setup::WorktreeSetupAttempt>,
    pub(crate) remote_history: bool,
    pub(crate) continuation: Option<conversation::PendingContinuation>,
    /// A project default for a chat that has never established an agent session.
    pub initial_model: Option<String>,
    pub chat: ChatRecord,
    pub next_seq: u32,
    pub next_turn_ordinal: u32,
    pub agent_session: Option<AgentSessionRow>,
    pub queue: Vec<QueuedRow>,
    pub recovered_turns: u32,
    pub replay_check: Option<ReplayCheck>,
    /// The session this chat's terminal last used, for an agent whose Chat and terminal keep
    /// sessions apart (Cursor, Hermes): this device's own.
    pub terminal_session: Option<TerminalSessionRecord>,
}

/// A terminal's own vendor session: the connection it ran through and the session's ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct TerminalSessionRecord {
    pub connection: String,
    pub session: String,
}

/// The device setting that keeps a chat's terminal session.
pub(crate) fn terminal_session_key(device: &str, chat_id: &str) -> String {
    format!("device/{device}/terminal-session/{chat_id}")
}

/// For the Hermes replay comparison: the latest agent session's turn count and its newest reply's
/// hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReplayCheck {
    pub native_session_id: String,
    pub turns: u32,
    pub last_reply_sha256: Option<String>,
}

/// An `agent_sessions` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentSessionRow {
    pub capabilities: Option<o7_protocol::Capabilities>,
    pub host: o7_protocol::agents::AgentHost,
    pub id: String,
    pub chat_id: String,
    pub agent_id: String,
    pub native_session_id: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub autonomy: Autonomy,
    /// The user's explicit option choices, JSON `{option_id: ConfigValue}`.
    pub config_json: Option<String>,
    pub controls_json: Option<String>,
    pub cwd: PathBuf,
    pub created_at: Millis,
    pub updated_at: Millis,
}

/// The fields of an [`AgentSessionRow`] to change; `None` keeps a field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentSessionPatch {
    pub capabilities: Option<o7_protocol::Capabilities>,
    pub native_session_id: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub autonomy: Option<Autonomy>,
    pub config_json: Option<String>,
    pub controls_json: Option<String>,
    pub updated_at: Millis,
}

/// A `turns` row as it starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TurnRow {
    pub continuation_prefix: Option<String>,
    pub checkpoint_ref: Option<String>,
    pub parts: Vec<o7_protocol::PromptPart>,
    pub id: String,
    pub chat_id: String,
    pub agent_session_id: Option<String>,
    /// v1's `turns.seq`: 1-based per chat, unique.
    pub ordinal: u32,
    pub prompt_id: String,
    pub first_event_seq: u32,
    pub started_at: Millis,
}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TurnEnd {
    pub turn_id: String,
    pub status: TurnStatus,
    pub stop_reason: StopReason,
    pub ended_at: Millis,
    pub last_event_seq: u32,
}

/// `turns.status`, v1's CHECK: `EndTurn`, `MaxTokens`, `MaxTurnRequests`, `Refusal`, `Cancelled`
/// and `Other` complete a turn; `Interrupted` interrupts it; `Error` fails it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Running,
    Completed,
    Interrupted,
    Failed,
}

/// One stored event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EventRow {
    pub seq: u32,
    pub turn_id: Option<String>,
    /// [`o7_protocol::O7Event::kind`].
    pub kind: &'static str,
    /// The event's JSON.
    pub payload: String,
    pub created_at: Millis,
}

/// A `queued_prompts` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QueuedRow {
    pub parts: Vec<o7_protocol::PromptPart>,
    pub id: String,
    pub chat_id: String,
    pub text: String,
    /// Queued before a stop, error, crash or restart: sent only after the user says so.
    pub held: bool,
    pub created_at: Millis,
}

/// `chats.title_source`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TitleSource {
    FirstLine,
    Agent,
    Manual,
}

/// How long startup's work on the database took, for the diagnostics card: the next time opening
/// the app is slow, this says where.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StartupTimings {
    /// Startup recovery, the writer's first op, which every first read waits behind. `None`
    /// until it finished.
    pub recovery_ms: Option<u64>,
    /// Building the indexes recovery reads through, after startup. `None` when this start built
    /// none, as every start but the first with them does.
    pub indexes_ms: Option<u64>,
}

/// What recovery did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RecoveryReport {
    pub interrupted_turns: u32,
    pub resolved_requests: u32,
    pub compacted_turns: u32,
}

/// Why a chat store call failed. Every message is a sentence the UI can show.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("The disk is full, so o7 paused saving. Free some space, then try again.")]
    DiskFull,
    #[error("The database is busy.")]
    Busy,
    #[error("o7's database is closed.")]
    Closed,
    #[error("o7's database is damaged: {0}")]
    Damaged(String),
    /// A blob that is not available, and the sentence that says why: "The full output is no longer
    /// available." for a file that is gone, "The full output isn't saved yet because the disk is
    /// full." for one still waiting in the failed batch or the FIFO. The session manager passes the
    /// sentence on as `SessionError::BlobMissing`.
    #[error("{0}")]
    BlobMissing(String),
    #[error("{0}")]
    Other(String),
}

/// The writer thread is gone: nothing more will be saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("o7's database is closed.")]
pub struct StoreClosed;

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use o7_protocol::session::{PauseReason, StorageState};
    use o7_protocol::{NoticeLevel, O7Event};
    use rusqlite::{Connection, params};
    use sha2::{Digest, Sha256};

    use super::test_support::event_row;
    use super::*;
    use crate::paths::DataRoot;
    use crate::profile::Profiles;

    /// A started chat store in a temporary profile.
    struct Fixture {
        _dir: tempfile::TempDir,
        profile: Profile,
        store: Arc<ChatStore>,
    }

    fn profile(dir: &Path) -> Profile {
        Profiles::new(DataRoot::at(dir.join("data")))
            .open_active()
            .expect("profile")
    }

    fn start_in(
        dir: tempfile::TempDir,
        config: WriterConfig,
        prepare: impl FnOnce(&Store),
    ) -> Fixture {
        let profile = profile(dir.path());
        let owner = ProfileOwner::acquire(&profile).expect("owner");
        let (store, _) = Store::open(&profile).expect("store");
        prepare(&store);
        let chats = ChatStore::start_with(
            store,
            owner,
            &profile,
            "dev".into(),
            Handle::current(),
            config,
        )
        .expect("start");
        Fixture {
            _dir: dir,
            profile,
            store: chats,
        }
    }

    fn start(config: WriterConfig) -> Fixture {
        start_in(tempfile::tempdir().expect("tempdir"), config, |_| {})
    }

    fn notice(seq: u32, text: &str) -> EventRow {
        event_row(
            seq,
            None,
            &O7Event::Notice {
                level: NoticeLevel::Info,
                message: text.into(),
            },
        )
    }

    async fn send(store: &ChatStore, op: Op) {
        store
            .reserve(op_bytes(&op))
            .await
            .expect("room in the FIFO")
            .send(op);
    }

    async fn new_chat(store: &ChatStore) -> ChatRecord {
        store
            .create_chat(NewChat {
                agent_host: Default::default(),
                agent_id: "mock".into(),
                folder: PathBuf::from("/work/repo"),
                device_id: "dev".into(),
                now: 5,
            })
            .await
            .expect("create")
    }

    fn page_seqs(page: &HistoryPage) -> Vec<u32> {
        page.items.iter().map(|item| item.seq).collect()
    }

    /// The newest page below `before`, up to 4 MiB.
    async fn page(store: &ChatStore, chat: &str, before: u32) -> HistoryPage {
        let (reply, answer) = oneshot::channel();
        send(
            store,
            Op::ReadPage {
                chat_id: chat.into(),
                before_seq: before,
                max_bytes: 4 * 1024 * 1024,
                reply,
            },
        )
        .await;
        await_reply(&Handle::current(), answer, Duration::from_secs(10))
            .await
            .expect("answer")
            .expect("page")
    }

    async fn status_until(
        store: &ChatStore,
        matches: impl Fn(&StorageStatus) -> bool,
    ) -> StorageStatus {
        let mut status = store.status();
        tokio::time::timeout(
            Duration::from_secs(20),
            status.wait_for(|status| matches(status)),
        )
        .await
        .expect("the status changes in time")
        .expect("status")
        .clone()
    }

    /// Holds the writer until the returned sender is dropped: it serves the second channel first,
    /// so every op queued meanwhile lands in one batch, whatever the scheduling.
    fn hold(store: &ChatStore) -> std::sync::mpsc::Sender<()> {
        let (release, wait) = std::sync::mpsc::channel::<()>();
        store.with_store(move |_| {
            let _ = wait.recv_timeout(Duration::from_secs(30));
        });
        release
    }

    fn dead_letters(profile: &Profile) -> Vec<serde_json::Value> {
        let mut lines = Vec::new();
        for entry in std::fs::read_dir(profile.backups_dir())
            .into_iter()
            .flatten()
            .flatten()
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("deadletter-") {
                let text = std::fs::read_to_string(entry.path()).expect("read");
                lines.extend(
                    text.lines()
                        .map(|line| serde_json::from_str(line).expect("json line")),
                );
            }
        }
        lines
    }

    #[tokio::test]
    async fn flush_commits_earlier_ops_and_a_timeout_does_not_close_the_writer() {
        let fixture = start(WriterConfig::STANDARD);
        let store = &fixture.store;
        let chat = new_chat(store).await;
        let held = hold(store);
        send(
            store,
            Op::AppendEvents {
                chat_id: chat.id.clone(),
                rows: vec![notice(0, "saved before switch")],
            },
        )
        .await;
        assert!(store.flush(Duration::from_millis(10)).await.is_err());
        drop(held);
        store
            .flush(Duration::from_secs(5))
            .await
            .expect("writer resumes without closing");
        let read = Connection::open_with_flags(
            fixture.profile.db_path(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .expect("independent reader");
        let count: u32 = read
            .query_row(
                "SELECT count(*) FROM events WHERE chat_id=?1",
                [&chat.id],
                |row| row.get(0),
            )
            .expect("durable rows");
        assert_eq!(count, 1);
        drop(read);
        assert!(store.shutdown(Duration::from_secs(5)).await);
        assert!(matches!(
            store.flush(Duration::from_secs(1)).await,
            Err(StoreError::Closed)
        ));
    }

    /// Line edits are counted on a connection of their own and saved through the writer, once.
    #[tokio::test]
    async fn line_edits_count_ended_turns_off_the_writer_and_save_them_once() {
        let fixture = start_in(
            tempfile::tempdir().expect("tempdir"),
            WriterConfig::STANDARD,
            |store| {
                let chat = rows::create_chat(
                    store.conn(),
                    &NewChat {
                        agent_host: Default::default(),
                        agent_id: "mock".into(),
                        folder: PathBuf::from("/work/repo"),
                        device_id: "dev".into(),
                        now: 1,
                    },
                )
                .expect("chat");
                store
                    .conn()
                    .execute(
                        "INSERT INTO turns (id, chat_id, seq, status, started_at, device_id, \
                         first_event_seq) VALUES ('t1', ?1, 1, 'completed', 1, 'dev', 0)",
                        [&chat.id],
                    )
                    .expect("turn");
                let edit = O7Event::ToolCall(o7_protocol::ToolCall {
                    tool_call_id: "t1.e".into(),
                    kind: o7_protocol::ToolKind::Edit,
                    title: "Write a.rs".into(),
                    status: o7_protocol::ToolStatus::Completed,
                    locations: Vec::new(),
                    raw_input: Some(
                        serde_json::json!({"file_path": "a.rs", "content": "a\nb\nc\n"}),
                    ),
                    tool_name: Some("Write".into()),
                    content: Vec::new(),
                    truncated: None,
                });
                rows::append_events(
                    store.conn(),
                    "dev",
                    &chat.id,
                    &[EventRow {
                        created_at: 1_800_000_000_000,
                        ..event_row(0, Some("t1"), &edit)
                    }],
                )
                .expect("event");
            },
        );
        let store = &fixture.store;
        let edits = store.line_edits(0).await.expect("line edits");
        assert!(edits.complete);
        assert_eq!(
            edits.buckets,
            vec![o7_protocol::activity::LineEditBucket {
                agent: "mock".into(),
                at: 1_800_000_000_000 - 1_800_000_000_000 % line_edits::BUCKET_MS,
                lines: 3,
            }]
        );
        // Counted once: asking again counts nothing more.
        assert_eq!(store.line_edits(0).await.expect("again"), edits);
        // A terminal's edits, from the agent's own record, land through the FIFO.
        let chat_id = store.chats().await.expect("chats")[0].id.clone();
        send(
            store,
            Op::RecordLineEdits {
                chat_id,
                turn_id: "t1".into(),
                edits: vec![(1_800_000_000_000 + line_edits::BUCKET_MS, 4)],
            },
        )
        .await;
        let recorded = store.line_edits(0).await.expect("recorded");
        assert_eq!(
            recorded
                .buckets
                .iter()
                .map(|bucket| bucket.lines)
                .collect::<Vec<_>>(),
            vec![3, 4]
        );
        assert!(
            store
                .line_edits(1_800_000_000_000 + 2 * line_edits::BUCKET_MS)
                .await
                .expect("later")
                .buckets
                .is_empty()
        );
        assert!(store.shutdown(Duration::from_secs(5)).await);
    }

    #[tokio::test]
    async fn composer_drafts_survive_a_profile_reopen() {
        use composer::ComposerCommand;
        let fixture = start(WriterConfig::STANDARD);
        let chat = new_chat(&fixture.store).await;
        let input = o7_protocol::composer::ComposerInput {
            text: "Unsent work".into(),
            attachments: Vec::new(),
            mentions: vec!["src/main.rs".into()],
        };
        fixture
            .store
            .composer_command(ComposerCommand::SaveDraft {
                chat_id: chat.id.clone(),
                input: input.clone(),
            })
            .await
            .expect("save acknowledged");
        assert!(fixture.store.shutdown(Duration::from_secs(10)).await);
        let (store, _) = Store::open(&fixture.profile).expect("reopen");
        let json: String = store
            .conn()
            .query_row(
                "SELECT input_json FROM composer_drafts WHERE chat_id=?1",
                [&chat.id],
                |row| row.get(0),
            )
            .expect("stored draft");
        assert_eq!(
            serde_json::from_str::<o7_protocol::composer::ComposerInput>(&json)
                .expect("typed draft"),
            input
        );
    }

    #[tokio::test]
    async fn composer_attachment_ids_are_chat_scoped_and_blobs_are_verified() {
        use composer::ComposerCommand;
        use sha2::{Digest as _, Sha256};
        let fixture = start(WriterConfig::STANDARD);
        let store = &fixture.store;
        let first = new_chat(store).await;
        let second = new_chat(store).await;
        let bytes: Arc<[u8]> = Arc::from(b"explicit context".as_slice());
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let saved = store
            .composer(ComposerCommand::Import {
                chat_id: first.id.clone(),
                name: "notes.txt".into(),
                mime: "text/plain".into(),
                sha256: hash.clone(),
                bytes,
            })
            .await
            .expect("saved");
        let id = saved[0].meta.id.clone();
        assert!(
            store
                .composer(ComposerCommand::Resolve {
                    chat_id: second.id,
                    ids: vec![id.clone()]
                })
                .await
                .is_err()
        );
        assert_eq!(
            store
                .composer(ComposerCommand::Resolve {
                    chat_id: first.id,
                    ids: vec![id]
                })
                .await
                .expect("owned")[0]
                .sha256,
            hash
        );
        let dir = fixture.profile.attachments_dir();
        assert_eq!(
            o7_drivers::attachments::read(&dir, &hash, 256).expect("verified"),
            b"explicit context"
        );
        std::fs::write(dir.join(&hash), b"changed data").expect("alter synthetic blob");
        assert!(o7_drivers::attachments::read(&dir, &hash, 256).is_err());
        assert!(store.shutdown(Duration::from_secs(10)).await);
    }

    #[tokio::test]
    async fn create_list_and_load_chats_through_the_fifo() {
        let fixture = start(WriterConfig::STANDARD);
        let store = &fixture.store;
        assert_eq!(store.recovered().await, RecoveryReport::default());
        let chat = new_chat(store).await;
        assert_eq!(
            store.chats().await.expect("chats"),
            std::slice::from_ref(&chat)
        );
        let load = store.load_chat(&chat.id).await.expect("load");
        assert_eq!(
            (load.chat, load.next_seq, load.next_turn_ordinal),
            (chat, 0, 1)
        );
        assert!(matches!(
            store.load_chat("nope").await,
            Err(StoreError::Other(_))
        ));
        assert_eq!(store.load_chat_inner("nope").await.expect("load"), None);
        assert!(!store.is_paused());
        assert_eq!(*store.status().borrow(), StorageStatus::default());
    }

    /// Ordering: a read queued behind writes sees all of them.
    #[tokio::test]
    async fn reads_see_every_earlier_write() {
        let fixture = start(WriterConfig::STANDARD);
        let store = &fixture.store;
        let chat = new_chat(store).await;
        for seq in 0..50 {
            send(
                store,
                Op::AppendEvents {
                    chat_id: chat.id.clone(),
                    rows: vec![notice(seq, "x")],
                },
            )
            .await;
        }
        let page = page(store, &chat.id, 50).await;
        assert_eq!(page_seqs(&page), (0..50).collect::<Vec<_>>());
    }

    /// Group commit: a burst queued while the writer is busy commits in a few transactions.
    #[tokio::test]
    async fn writer_group_commits_a_burst() {
        let fixture = start(WriterConfig::STANDARD);
        let store = &fixture.store;
        let chat = new_chat(store).await;
        assert!(store.barrier().await);
        let before = store
            .shared
            .commits
            .load(std::sync::atomic::Ordering::Relaxed);
        let held = hold(store);
        for seq in 0..600 {
            send(
                store,
                Op::AppendEvents {
                    chat_id: chat.id.clone(),
                    rows: vec![notice(seq, "burst")],
                },
            )
            .await;
        }
        drop(held);
        assert!(store.barrier().await);
        let commits = store
            .shared
            .commits
            .load(std::sync::atomic::Ordering::Relaxed)
            - before;
        assert!(commits <= 4, "{commits} transactions for 600 ops");
        assert_eq!(page(store, &chat.id, 600).await.items.len(), 600);
    }

    /// The FIFO is bounded: with a stalled writer, the 1,025th reservation waits.
    #[tokio::test]
    async fn the_fifo_is_bounded_by_ops_and_bytes() {
        let config = WriterConfig {
            fifo_ops: 4,
            fifo_bytes: 4096,
            ..WriterConfig::STANDARD
        };
        let fixture = start(config);
        let store = &fixture.store;
        let (release, wait) = std::sync::mpsc::channel::<()>();
        store.with_store(move |_| {
            let _ = wait.recv_timeout(Duration::from_secs(10));
        });
        let mut held = Vec::new();
        for _ in 0..4 {
            held.push(store.reserve(10).await.expect("room"));
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(100), store.reserve(10))
                .await
                .is_err(),
            "the fifth op waits for room"
        );
        drop(held);
        let big = store
            .reserve(10_000)
            .await
            .expect("one op larger than the byte bound passes alone");
        assert!(
            tokio::time::timeout(Duration::from_millis(100), store.reserve(10))
                .await
                .is_err()
        );
        drop(big);
        let _ = release.send(());
    }

    fn queued_prompt(chat: &str) -> Op {
        Op::QueuePut(QueuedRow {
            parts: Vec::new(),
            id: "p1".into(),
            chat_id: chat.into(),
            text: "hello".into(),
            held: false,
            created_at: 1,
        })
    }

    /// A `BeginTurn` whose second row reuses seq 0, so UNIQUE (chat_id, seq) fails inside the op.
    fn failing_begin_turn(chat: &str) -> Op {
        let started = O7Event::TurnStarted {
            turn_id: "t1".into(),
            prompt_id: Some("p1".into()),
        };
        Op::BeginTurn {
            turn: TurnRow {
                continuation_prefix: None,
                checkpoint_ref: None,
                parts: Vec::new(),
                id: "t1".into(),
                chat_id: chat.into(),
                agent_session_id: None,
                ordinal: 1,
                prompt_id: "p1".into(),
                first_event_seq: 1,
                started_at: 2,
            },
            rows: vec![
                event_row(1, Some("t1"), &started),
                event_row(0, Some("t1"), &started),
            ],
        }
    }

    /// An op is atomic: a `BeginTurn` that fails inside its batch leaves neither its turn nor its
    /// rows and keeps the queued prompt, while the ops around it commit.
    #[tokio::test]
    async fn ops_are_atomic() {
        let fixture = start(WriterConfig::STANDARD);
        let store = &fixture.store;
        let chat = new_chat(store).await;
        send(store, queued_prompt(&chat.id)).await;
        let held = hold(store);
        send(
            store,
            Op::AppendEvents {
                chat_id: chat.id.clone(),
                rows: vec![notice(0, "first")],
            },
        )
        .await;
        send(store, failing_begin_turn(&chat.id)).await;
        send(
            store,
            Op::AppendEvents {
                chat_id: chat.id.clone(),
                rows: vec![notice(2, "last")],
            },
        )
        .await;
        drop(held);
        assert!(store.barrier().await);
        assert_eq!(page_seqs(&page(store, &chat.id, 10).await), [0, 2]);
        let load = store.load_chat(&chat.id).await.expect("load");
        assert_eq!(load.next_turn_ordinal, 1, "no turn row");
        assert_eq!(load.queue.len(), 1, "the prompt is still queued");
        let dead = dead_letters(&fixture.profile);
        assert_eq!(dead.len(), 1);
        assert_eq!(dead[0]["op"]["op"], "begin_turn");
        assert!(
            dead[0]["error"]
                .as_str()
                .is_some_and(|error| error.contains("UNIQUE")),
            "{dead:?}"
        );
    }

    /// S6: a poison op (rows for a chat that does not exist) goes to the dead-letter file as JSON;
    /// the rest of its batch commits.
    #[tokio::test]
    async fn poison_op_is_isolated() {
        let fixture = start(WriterConfig::STANDARD);
        let store = &fixture.store;
        let chat = new_chat(store).await;
        let held = hold(store);
        send(
            store,
            Op::AppendEvents {
                chat_id: chat.id.clone(),
                rows: vec![notice(0, "before")],
            },
        )
        .await;
        send(
            store,
            Op::AppendEvents {
                chat_id: "missing".into(),
                rows: vec![notice(0, "poison")],
            },
        )
        .await;
        send(
            store,
            Op::AppendEvents {
                chat_id: chat.id.clone(),
                rows: vec![notice(1, "after")],
            },
        )
        .await;
        drop(held);
        assert!(store.barrier().await);
        assert_eq!(page_seqs(&page(store, &chat.id, 10).await), [0, 1]);
        let dead = dead_letters(&fixture.profile);
        assert_eq!(dead.len(), 1);
        assert_eq!(dead[0]["op"]["op"], "append_events");
        assert_eq!(dead[0]["op"]["chat_id"], "missing");
        assert_eq!(dead[0]["op"]["rows"][0]["seq"], 0);
        assert!(dead[0]["at"].as_i64().is_some_and(|at| at > 0));
        assert_eq!(
            *store.status().borrow(),
            StorageStatus::default(),
            "saving goes on"
        );
    }

    /// S5: a panic inside a batch is rolled back and isolated, and the thread continues.
    #[tokio::test]
    async fn writer_survives_panicking_batch() {
        let fixture = start(WriterConfig::STANDARD);
        let store = &fixture.store;
        let chat = new_chat(store).await;
        let held = hold(store);
        send(
            store,
            Op::AppendEvents {
                chat_id: chat.id.clone(),
                rows: vec![notice(0, "a")],
            },
        )
        .await;
        store.test_work(TestWork::Panic);
        send(
            store,
            Op::AppendEvents {
                chat_id: chat.id.clone(),
                rows: vec![notice(1, "b")],
            },
        )
        .await;
        drop(held);
        assert!(store.barrier().await);
        send(
            store,
            Op::AppendEvents {
                chat_id: chat.id.clone(),
                rows: vec![notice(2, "after")],
            },
        )
        .await;
        assert_eq!(page_seqs(&page(store, &chat.id, 10).await), [0, 1, 2]);
        let dead = dead_letters(&fixture.profile);
        assert_eq!(dead.len(), 1);
        assert_eq!(dead[0]["op"]["op"], "test_panic");
        assert_eq!(*store.status().borrow(), StorageStatus::default());
    }

    fn sha(bytes: &[u8]) -> String {
        rows::hex(&Sha256::digest(bytes))
    }

    /// S3 (store part) and S8: a full disk pauses saving; the writer keeps the failed batch and takes
    /// no ops; reads are served from saved data with what is unsaved marked; freeing space resumes
    /// and nothing is lost.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn disk_full_pauses_without_loss() {
        let config = WriterConfig {
            disk_full_retry: Duration::from_millis(50),
            ..WriterConfig::STANDARD
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let fixture = start_in(dir, config, |_| {});
        let store = &fixture.store;
        let chat = new_chat(store).await;
        send(
            store,
            Op::AppendEvents {
                chat_id: chat.id.clone(),
                rows: vec![notice(0, "saved")],
            },
        )
        .await;
        assert!(store.barrier().await);
        // Leave room for almost nothing more.
        store.with_store(|store| {
            let pages: i64 = store
                .conn()
                .query_row("PRAGMA page_count", [], |row| row.get(0))
                .expect("pages");
            let _: i64 = store
                .conn()
                .query_row(
                    &format!("PRAGMA max_page_count = {}", pages + 1),
                    [],
                    |row| row.get(0),
                )
                .expect("cap");
        });
        let big = "y".repeat(40_000);
        let blob = Arc::<[u8]>::from(big.as_bytes());
        let blob_id = sha(&blob);
        // Everything below lands in one batch, which fails as a whole.
        let held = hold(store);
        send(
            store,
            Op::PutBlob {
                sha256: blob_id.clone(),
                mime: "text/plain".into(),
                bytes: blob,
            },
        )
        .await;
        for seq in 1..=20 {
            send(
                store,
                Op::AppendEvents {
                    chat_id: chat.id.clone(),
                    rows: vec![notice(seq, &big)],
                },
            )
            .await;
        }
        drop(held);
        let paused = status_until(store, |status| {
            matches!(status.state, StorageState::Paused { .. })
        })
        .await;
        assert!(matches!(
            paused.state,
            StorageState::Paused {
                reason: PauseReason::DiskFull,
                ..
            }
        ));
        assert!(store.is_paused());
        assert!(paused.unsaved_bytes > 0);

        // Reads never wait behind the paused writer.
        let started = std::time::Instant::now();
        assert_eq!(store.chats().await.expect("chats").len(), 1);
        let saved = store
            .read_saved_page(&chat.id, 21, history::DEFAULT_PAGE_BYTES)
            .await
            .expect("page");
        assert_eq!(page_seqs(&saved), [0]);
        assert_eq!(saved.unsaved_from, Some(1));
        assert_eq!(
            store.read_blob(&blob_id, 0, 100).await,
            Err(StoreError::BlobMissing(blobs::BLOB_UNSAVED.into()))
        );
        assert!(
            matches!(store.load_chat(&chat.id).await, Err(StoreError::DiskFull)),
            "it has unsaved ops"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );

        // Free space: the failed batch and everything behind it commit.
        store.with_store(|store| {
            let _: i64 = store
                .conn()
                .query_row("PRAGMA max_page_count = 1073741823", [], |row| row.get(0))
                .expect("uncap");
        });
        status_until(store, |status| status.state == StorageState::Ok).await;
        assert!(!store.is_paused());
        assert_eq!(
            page_seqs(&page(store, &chat.id, 21).await),
            (0..=20).collect::<Vec<_>>()
        );
        let text = store.read_blob(&blob_id, 0, 100).await.expect("blob saved");
        assert_eq!(text.total, 40_000);
    }

    /// S8: reads never wait behind a paused writer: while a full disk pauses saving, chat lists,
    /// saved pages (with the unsaved part marked) and blob reads answer at once; a read that was
    /// already queued behind the failed batch gives up after its bound with `DiskFull`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reads_never_wait_behind_a_paused_writer() {
        let config = WriterConfig {
            disk_full_retry: Duration::from_millis(50),
            ..WriterConfig::STANDARD
        };
        let fixture = start(config);
        let store = Arc::clone(&fixture.store);
        let chat = new_chat(&store).await;
        send(
            &store,
            Op::AppendEvents {
                chat_id: chat.id.clone(),
                rows: vec![notice(0, "saved")],
            },
        )
        .await;
        assert!(store.barrier().await);
        store.with_store(|store| {
            let pages: i64 = store
                .conn()
                .query_row("PRAGMA page_count", [], |row| row.get(0))
                .expect("pages");
            let _: i64 = store
                .conn()
                .query_row(
                    &format!("PRAGMA max_page_count = {}", pages + 1),
                    [],
                    |row| row.get(0),
                )
                .expect("cap");
        });
        // Hold the writer so the writes and the read behind them land in one batch.
        let held = hold(&store);
        let big = "w".repeat(60_000);
        let blob = Arc::<[u8]>::from(big.as_bytes());
        let blob_id = sha(&blob);
        send(
            &store,
            Op::PutBlob {
                sha256: blob_id.clone(),
                mime: "text/plain".into(),
                bytes: blob,
            },
        )
        .await;
        for seq in 1..=3 {
            send(
                &store,
                Op::AppendEvents {
                    chat_id: chat.id.clone(),
                    rows: vec![notice(seq, &big)],
                },
            )
            .await;
        }
        let queued_read = tokio::spawn({
            let store = Arc::clone(&store);
            async move { store.chats().await }
        });
        let read_queued = tokio::time::timeout(Duration::from_secs(10), async {
            while store.shared.queued() < 5 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await;
        assert!(read_queued.is_ok(), "the read waits behind the writes");
        drop(held);
        status_until(&store, |status| {
            matches!(status.state, StorageState::Paused { .. })
        })
        .await;

        let started = std::time::Instant::now();
        assert_eq!(store.chats().await.expect("chats").len(), 1);
        let saved = store
            .read_saved_page(&chat.id, 10, history::DEFAULT_PAGE_BYTES)
            .await
            .expect("page");
        assert_eq!(page_seqs(&saved), [0]);
        assert_eq!(saved.unsaved_from, Some(1));
        assert_eq!(
            store.read_blob(&blob_id, 0, 100).await,
            Err(StoreError::BlobMissing(blobs::BLOB_UNSAVED.into()))
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );

        let queued = queued_read.await.expect("join");
        assert_eq!(
            queued,
            Err(StoreError::DiskFull),
            "queued before the pause: gives up after 5 s"
        );
        store.with_store(|store| {
            let _: i64 = store
                .conn()
                .query_row("PRAGMA max_page_count = 1073741823", [], |row| row.get(0))
                .expect("uncap");
        });
        status_until(&store, |status| status.state == StorageState::Ok).await;
    }

    /// S8: while paused, a chat with nothing unsaved loads from saved data, its queue held.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn attach_while_paused_reads_saved_rows() {
        let config = WriterConfig {
            disk_full_retry: Duration::from_millis(50),
            ..WriterConfig::STANDARD
        };
        let fixture = start(config);
        let store = &fixture.store;
        let quiet = new_chat(store).await;
        let busy = new_chat(store).await;
        send(
            store,
            Op::QueuePut(QueuedRow {
                parts: Vec::new(),
                id: "q".into(),
                chat_id: quiet.id.clone(),
                text: "next".into(),
                held: false,
                created_at: 1,
            }),
        )
        .await;
        send(
            store,
            Op::AppendEvents {
                chat_id: quiet.id.clone(),
                rows: vec![notice(0, "old")],
            },
        )
        .await;
        assert!(store.barrier().await);
        store.with_store(|store| {
            let pages: i64 = store
                .conn()
                .query_row("PRAGMA page_count", [], |row| row.get(0))
                .expect("pages");
            let _: i64 = store
                .conn()
                .query_row(
                    &format!("PRAGMA max_page_count = {}", pages + 1),
                    [],
                    |row| row.get(0),
                )
                .expect("cap");
        });
        let big = "z".repeat(60_000);
        for seq in 0..10 {
            send(
                store,
                Op::AppendEvents {
                    chat_id: busy.id.clone(),
                    rows: vec![notice(seq, &big)],
                },
            )
            .await;
        }
        status_until(store, |status| {
            matches!(status.state, StorageState::Paused { .. })
        })
        .await;
        let load = store
            .load_chat(&quiet.id)
            .await
            .expect("served from saved data");
        assert_eq!(load.next_seq, 1);
        assert_eq!(load.queue.len(), 1);
        assert!(load.queue[0].held, "reads as held");
        let page = store
            .read_saved_page(&quiet.id, 1, history::DEFAULT_PAGE_BYTES)
            .await
            .expect("page");
        assert_eq!((page_seqs(&page), page.unsaved_from), (vec![0], None));
        store.with_store(|store| {
            let _: i64 = store
                .conn()
                .query_row("PRAGMA max_page_count = 1073741823", [], |row| row.get(0))
                .expect("uncap");
        });
        status_until(store, |status| status.state == StorageState::Ok).await;
    }

    /// S25 (store part): quitting while paused gives up at the deadline, logs what it drops, and the
    /// next start is consistent.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn quit_while_paused_drops_only_what_was_unsaved() {
        let config = WriterConfig {
            disk_full_retry: Duration::from_millis(50),
            ..WriterConfig::STANDARD
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let fixture = start_in(dir, config, |_| {});
        let store = Arc::clone(&fixture.store);
        let chat = new_chat(&store).await;
        send(
            &store,
            Op::AppendEvents {
                chat_id: chat.id.clone(),
                rows: vec![notice(0, "kept")],
            },
        )
        .await;
        assert!(store.barrier().await);
        store.with_store(|store| {
            let pages: i64 = store
                .conn()
                .query_row("PRAGMA page_count", [], |row| row.get(0))
                .expect("pages");
            let _: i64 = store
                .conn()
                .query_row(
                    &format!("PRAGMA max_page_count = {}", pages + 1),
                    [],
                    |row| row.get(0),
                )
                .expect("cap");
        });
        let big = "q".repeat(60_000);
        for seq in 1..=5 {
            send(
                &store,
                Op::AppendEvents {
                    chat_id: chat.id.clone(),
                    rows: vec![notice(seq, &big)],
                },
            )
            .await;
        }
        status_until(&store, |status| {
            matches!(status.state, StorageState::Paused { .. })
        })
        .await;
        let started = std::time::Instant::now();
        assert!(
            !store.shutdown(Duration::from_millis(300)).await,
            "something was lost"
        );
        assert!(started.elapsed() < Duration::from_secs(3));
        drop(store);
        // The next start reads a consistent database with what was saved.
        let profile = profile(&root);
        let (reopened, info) = Store::open(&profile).expect("reopen");
        assert!(!info.unclean_shutdown);
        let count: i64 = reopened
            .conn()
            .query_row(
                "SELECT count(*) FROM events WHERE chat_id = ?1",
                params![chat.id],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(count, 1);
    }

    /// S36 (store part): a quit with ops still queued commits them all, folds the WAL back, and the
    /// next launch reports a clean shutdown; the owner lock goes with the store.
    #[tokio::test]
    async fn quit_without_sessions_commits_and_closes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let fixture = start_in(dir, WriterConfig::STANDARD, |_| {});
        let store = Arc::clone(&fixture.store);
        let chat = new_chat(&store).await;
        for seq in 0..300 {
            send(
                &store,
                Op::AppendEvents {
                    chat_id: chat.id.clone(),
                    rows: vec![notice(seq, "queued")],
                },
            )
            .await;
        }
        assert!(store.shutdown(Duration::from_secs(5)).await);
        assert!(
            store.shutdown_blocking(Duration::from_secs(1)),
            "a second call reports the same"
        );
        assert!(store.reserve(1).await.is_err(), "closed for new ops");
        let profile = profile(&root);
        ProfileOwner::acquire(&profile).expect("the owner lock was released");
        let wal = crate::fsutil::with_suffix(&profile.db_path(), "-wal");
        assert!(
            std::fs::metadata(&wal).map_or(true, |meta| meta.len() == 0),
            "the WAL was folded back"
        );
        let (reopened, info) = Store::open(&profile).expect("reopen");
        assert!(!info.unclean_shutdown);
        let count: i64 = reopened
            .conn()
            .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
            .expect("count");
        assert_eq!(count, 300);
    }

    /// `shutdown_blocking` works from a plain thread with no runtime, and never panics.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_blocking_works_without_a_runtime() {
        let fixture = start(WriterConfig::STANDARD);
        let store = Arc::clone(&fixture.store);
        let chat = new_chat(&store).await;
        send(
            &store,
            Op::AppendEvents {
                chat_id: chat.id.clone(),
                rows: vec![notice(0, "x")],
            },
        )
        .await;
        let saved = std::thread::spawn(move || store.shutdown_blocking(Duration::from_secs(5)))
            .join()
            .expect("no panic");
        assert!(saved);
    }

    /// Busy past the timeout retries with backoff, then pauses; once the other writer lets go,
    /// saving resumes with nothing lost.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_busy_database_retries_then_pauses_then_resumes() {
        let config = WriterConfig {
            busy_backoff_first: Duration::from_millis(10),
            busy_backoff_max: Duration::from_millis(40),
            busy_pause_after: Duration::from_millis(200),
            ..WriterConfig::STANDARD
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = profile(dir.path()).db_path();
        let fixture = start_in(dir, config, |store| {
            store
                .conn()
                .busy_timeout(Duration::from_millis(20))
                .expect("busy timeout");
        });
        let store = &fixture.store;
        let chat = new_chat(store).await;
        let other = Connection::open(&db_path).expect("another tool");
        other
            .execute_batch("BEGIN IMMEDIATE")
            .expect("hold the write lock");
        send(
            store,
            Op::AppendEvents {
                chat_id: chat.id.clone(),
                rows: vec![notice(0, "waits")],
            },
        )
        .await;
        let paused = status_until(store, |status| {
            matches!(status.state, StorageState::Paused { .. })
        })
        .await;
        assert!(matches!(
            paused.state,
            StorageState::Paused {
                reason: PauseReason::Busy,
                ..
            }
        ));
        other.execute_batch("ROLLBACK").expect("let go");
        status_until(store, |status| status.state == StorageState::Ok).await;
        assert_eq!(page_seqs(&page(store, &chat.id, 1).await), [0]);
    }

    /// S8 under a busy pause, with the standard 5 s busy timeout: the first attempt waits it out,
    /// then the retries skip SQLite's busy handler, so reads from saved data answer at once between
    /// them instead of waiting behind each retry; the standard timeout is back once saving resumes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_busy_pause_never_holds_reads_behind_its_retries() {
        let config = WriterConfig {
            busy_backoff_first: Duration::from_millis(10),
            busy_backoff_max: Duration::from_millis(40),
            busy_pause_after: Duration::from_millis(100),
            ..WriterConfig::STANDARD
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = profile(dir.path()).db_path();
        let fixture = start_in(dir, config, |_| {});
        let store = &fixture.store;
        let chat = new_chat(store).await;
        let other = Connection::open(&db_path).expect("another tool");
        other
            .execute_batch("BEGIN IMMEDIATE")
            .expect("hold the write lock");
        send(
            store,
            Op::AppendEvents {
                chat_id: chat.id.clone(),
                rows: vec![notice(0, "waits")],
            },
        )
        .await;
        let paused = status_until(store, |status| {
            matches!(status.state, StorageState::Paused { .. })
        })
        .await;
        assert!(matches!(
            paused.state,
            StorageState::Paused {
                reason: PauseReason::Busy,
                ..
            }
        ));
        for _ in 0..5 {
            let started = std::time::Instant::now();
            assert_eq!(store.chats().await.expect("chats").len(), 1);
            let saved = store
                .read_saved_page(&chat.id, 1, history::DEFAULT_PAGE_BYTES)
                .await
                .expect("page");
            assert_eq!((saved.items.len(), saved.unsaved_from), (0, Some(0)));
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "a read waited behind a retry: {:?}",
                started.elapsed()
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        other.execute_batch("ROLLBACK").expect("let go");
        status_until(store, |status| status.state == StorageState::Ok).await;
        assert_eq!(page_seqs(&page(store, &chat.id, 1).await), [0]);
        let (timeout, read) = std::sync::mpsc::channel();
        store.with_store(move |store| {
            let ms: i64 = store
                .conn()
                .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
                .expect("busy timeout");
            let _ = timeout.send(ms);
        });
        assert_eq!(read.recv_timeout(Duration::from_secs(10)), Ok(5000));
    }

    /// S4: damage found during a run is recorded for the next open's repair, the status turns
    /// `Failed`, and later writes go to the dead-letter file.
    #[tokio::test]
    async fn corruption_switches_to_dead_letter() {
        let dir = tempfile::tempdir().expect("tempdir");
        let profile = profile(dir.path());
        let chat_id = {
            let (store, _) = Store::open(&profile).expect("store");
            let chat = rows::create_chat(
                store.conn(),
                &NewChat {
                    agent_host: Default::default(),
                    agent_id: "mock".into(),
                    folder: PathBuf::from("/w"),
                    device_id: "dev".into(),
                    now: 1,
                },
            )
            .expect("chat");
            chat.id
        };
        // Smash the chats table's page in the closed file; the cheap probe at open does not read it.
        let root: i64 = Connection::open(profile.db_path())
            .expect("raw")
            .query_row(
                "SELECT rootpage FROM sqlite_schema WHERE name = 'chats'",
                [],
                |row| row.get(0),
            )
            .expect("root page");
        let mut bytes = std::fs::read(profile.db_path()).expect("read");
        let start = usize::try_from((root - 1) * 4096).expect("offset");
        bytes[start..start + 64].fill(0xA5);
        std::fs::write(profile.db_path(), bytes).expect("write");

        let fixture = start_in(dir, WriterConfig::STANDARD, |_| {});
        let store = &fixture.store;
        assert!(matches!(store.chats().await, Err(StoreError::Damaged(_))));
        let failed = status_until(store, |status| {
            matches!(status.state, StorageState::Failed { .. })
        })
        .await;
        assert!(
            matches!(failed.state, StorageState::Failed { message } if message.contains("Restart o7"))
        );
        send(
            store,
            Op::AppendEvents {
                chat_id: chat_id.clone(),
                rows: vec![notice(0, "kept somewhere")],
            },
        )
        .await;
        assert!(store.barrier().await);
        let dead = dead_letters(&fixture.profile);
        assert_eq!(dead.len(), 1);
        assert_eq!(dead[0]["op"]["op"], "append_events");
        assert_eq!(dead[0]["op"]["rows"][0]["seq"], 0);
        assert!(store.shutdown(Duration::from_secs(5)).await);
        // The next open repairs what was recorded.
        let (_, info) = Store::open(&fixture.profile).expect("reopen");
        assert!(
            matches!(
                info.outcome,
                crate::store::OpenOutcome::RecoveredFromCorruption { .. }
            ),
            "{:?}",
            info.outcome
        );
    }

    /// Recovery is the writer's first op, and its report is available to callers.
    #[tokio::test]
    async fn startup_recovery_runs_first() {
        let dir = tempfile::tempdir().expect("tempdir");
        let fixture = start_in(dir, WriterConfig::STANDARD, |store| {
            let chat = rows::create_chat(
                store.conn(),
                &NewChat {
                    agent_host: Default::default(),
                    agent_id: "mock".into(),
                    folder: PathBuf::from("/w"),
                    device_id: "dev".into(),
                    now: 1,
                },
            )
            .expect("chat");
            store
                .conn()
                .execute(
                    "INSERT INTO turns (id, chat_id, seq, status, started_at, device_id, first_event_seq) \
                     VALUES ('t', ?1, 1, 'running', 1, 'dev', 0)",
                    params![chat.id],
                )
                .expect("turn");
        });
        // Nothing is timed before it happened, and this crate's tests leave the indexes unbuilt.
        let report = fixture.store.recovered().await;
        assert_eq!(report.interrupted_turns, 1);
        assert_eq!(report.compacted_turns, 1);
        let timings = fixture.store.startup_timings();
        assert!(timings.recovery_ms.is_some_and(|ms| ms < 60_000));
        assert_eq!(timings.indexes_ms, None);
    }

    /// A chat with one turn left running, holding a streamed answer of about 2 KiB.
    fn interrupted_turn(store: &Store) -> String {
        let chat = rows::create_chat(
            store.conn(),
            &NewChat {
                agent_host: Default::default(),
                agent_id: "mock".into(),
                folder: PathBuf::from("/w"),
                device_id: "dev".into(),
                now: 1,
            },
        )
        .expect("chat");
        store
            .conn()
            .execute(
                "INSERT INTO turns (id, chat_id, seq, status, started_at, device_id, first_event_seq) \
                 VALUES ('t', ?1, 1, 'running', 1, 'dev', 0)",
                params![chat.id],
            )
            .expect("turn");
        let rows: Vec<EventRow> = (0..40)
            .map(|seq| {
                event_row(
                    seq,
                    Some("t"),
                    &O7Event::MessageDelta {
                        message_id: "t.m1".into(),
                        text: "streamed ".into(),
                    },
                )
            })
            .collect();
        rows::append_events(store.conn(), "dev", &chat.id, &rows).expect("events");
        chat.id
    }

    /// Runs `read` on the writer's connection and waits for what it found.
    async fn on_store<T: Send + 'static>(
        store: &ChatStore,
        read: impl Fn(&Connection) -> T + Send + Sync + 'static,
    ) -> T {
        let (found, answer) = oneshot::channel();
        store.with_store(move |store| {
            let _ = found.send(read(store.conn()));
        });
        await_reply(&Handle::current(), answer, Duration::from_secs(10))
            .await
            .expect("the store answers")
    }

    /// Waits until the pass after startup compacted turn `t`; returns how many events it has.
    async fn compacted(store: &ChatStore) -> i64 {
        for _ in 0..400 {
            let (done, events) = on_store(store, |conn| {
                let done: bool = conn
                    .query_row(
                        "SELECT compacted_at IS NOT NULL FROM turns WHERE id = 't'",
                        [],
                        |row| row.get(0),
                    )
                    .expect("turn");
                let events: i64 = conn
                    .query_row(
                        "SELECT count(*) FROM events WHERE turn_id = 't'",
                        [],
                        |row| row.get(0),
                    )
                    .expect("events");
                (done, events)
            })
            .await;
            if done {
                return events;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("turn t was never compacted");
    }

    async fn searchable(store: &ChatStore) -> i64 {
        on_store(store, |conn| {
            conn.query_row(
                "SELECT count(*) FROM message_fts WHERE message_fts MATCH 'streamed'",
                [],
                |row| row.get(0),
            )
            .expect("search")
        })
        .await
    }

    /// Startup ends a large turn without compacting it; the pass after startup builds recovery's
    /// indexes and compacts the turn as ordinary queued work.
    #[tokio::test]
    async fn the_pass_after_startup_compacts_what_recovery_left() {
        let config = WriterConfig {
            leftovers: true,
            index_delay: Duration::ZERO,
            compact: compact::CompactLimits {
                inline_limit: 200,
                ..compact::CompactLimits::STANDARD
            },
            ..WriterConfig::STANDARD
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let fixture = start_in(dir, config, |store| {
            interrupted_turn(store);
        });
        let store = &fixture.store;
        let report = store.recovered().await;
        assert_eq!(
            (report.interrupted_turns, report.compacted_turns),
            (1, 0),
            "startup ends the turn and leaves its compaction"
        );
        // The stream of 40 pieces merges into one message, beside the end recovery appended.
        assert_eq!(compacted(store).await, 2);
        assert_eq!(searchable(store).await, 1);
        let indexes = on_store(store, |conn| {
            conn.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE type = 'index' \
                 AND name IN ('events_steering', 'events_turn_ended')",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("indexes")
        })
        .await;
        assert_eq!(indexes, 2);
        assert!(store.shutdown(Duration::from_secs(5)).await);
        // A store that carries those indexes opens as any other.
        let (reopened, info) = Store::open(&fixture.profile).expect("reopen");
        assert!(
            matches!(info.outcome, crate::store::OpenOutcome::Opened),
            "{:?}",
            info.outcome
        );
        drop(reopened);
    }

    /// A turn too large to merge keeps its rows: the pass reads its text on a connection of its
    /// own and saves only that, so the writer never reads the turn.
    #[tokio::test]
    async fn a_turn_too_large_to_merge_is_read_off_the_writer() {
        let config = WriterConfig {
            leftovers: true,
            // The indexes stay unbuilt here: the turn is compacted without them.
            index_delay: Duration::from_secs(3_600),
            compact: compact::CompactLimits {
                inline_limit: 200,
                merge_limit: 300,
                ..compact::CompactLimits::STANDARD
            },
            ..WriterConfig::STANDARD
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let fixture = start_in(dir, config, |store| {
            interrupted_turn(store);
        });
        let store = &fixture.store;
        assert_eq!(store.recovered().await.compacted_turns, 0);
        assert_eq!(compacted(store).await, 41, "its rows stay as they were");
        assert_eq!(searchable(store).await, 1);
        // The writer leaves such a turn alone when an actor asks, too.
        let chat = store.chats().await.expect("chats").remove(0);
        send(
            store,
            Op::CompactTurn {
                chat_id: chat.id,
                turn_id: "t".into(),
            },
        )
        .await;
        assert!(store.barrier().await);
        assert_eq!(compacted(store).await, 41);
        assert_eq!(searchable(store).await, 1, "and it is not stored twice");
    }

    /// A profile the size of a long-used one: `events` rows of about 1 KiB each over 40 chats whose
    /// turns ended and were compacted, and three chats that were mid-turn when the app quit, one of
    /// them with a turn far above the inline limit.
    fn large_profile(store: &Store, events: u32) {
        let conn = store.conn();
        let filler = super::test_support::payload(&O7Event::Notice {
            level: NoticeLevel::Info,
            message: "x".repeat(1000),
        });
        let chat = |folder: &str| {
            rows::create_chat(
                conn,
                &NewChat {
                    agent_host: Default::default(),
                    agent_id: "mock".into(),
                    folder: PathBuf::from(folder),
                    device_id: "dev".into(),
                    now: 1,
                },
            )
            .expect("chat")
            .id
        };
        let fill = |chat_id: &str, turn: &str, status: &str, compacted: bool, count: u32| {
            conn.execute(
                "INSERT INTO turns (id, chat_id, seq, status, started_at, device_id, first_event_seq, compacted_at) \
                 VALUES (?1, ?2, 1, ?3, 1, 'dev', 0, ?4)",
                params![turn, chat_id, status, compacted.then_some(1)],
            )
            .expect("turn");
            conn.execute(
                "WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i < ?4 - 1) \
                 INSERT INTO events (id, chat_id, turn_id, seq, kind, payload, created_at, device_id) \
                 SELECT ?2 || '-' || i, ?1, ?2, i, 'notice', ?3, 1, 'dev' FROM n",
                params![chat_id, turn, filler, count],
            )
            .expect("events");
        };
        conn.execute_batch("BEGIN").expect("begin");
        for index in 0..40 {
            let id = chat(&format!("/w/{index}"));
            fill(
                &id,
                &format!("done-{index}"),
                "completed",
                true,
                events / 40,
            );
        }
        for (index, count) in [(0, 40_000), (1, 400), (2, 40)] {
            let id = chat(&format!("/w/live-{index}"));
            fill(&id, &format!("live-{index}"), "running", false, count);
        }
        conn.execute_batch("COMMIT").expect("commit");
    }

    /// Measures startup on a large profile (`cargo test -p o7-core --lib large_profile -- --ignored
    /// --nocapture`; `O7_LARGE_EVENTS` sets its size, 300,000 events when unset). It prints what
    /// the search this version dropped costs on the same data, then holds startup recovery and the
    /// first list to a second on a first start (no indexes yet) and on the next (indexes built).
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "builds a database of hundreds of megabytes"]
    async fn startup_on_a_large_profile_does_not_read_its_events() {
        let events = std::env::var("O7_LARGE_EVENTS")
            .ok()
            .and_then(|count| count.parse().ok())
            .unwrap_or(300_000);
        let config = WriterConfig {
            leftovers: true,
            index_delay: Duration::from_millis(200),
            ..WriterConfig::STANDARD
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let built = std::time::Instant::now();
        // When the writer was started: recovery is its first op.
        let begun = Arc::new(std::sync::Mutex::new(std::time::Instant::now()));
        let mark = Arc::clone(&begun);
        let fixture = start_in(dir, config, move |store| {
            large_profile(store, events);
            eprintln!("built {events} events in {:?}", built.elapsed());
            let scan = std::time::Instant::now();
            let pending: i64 = store
                .conn()
                .query_row(
                    "SELECT count(*) FROM (SELECT DISTINCT e.chat_id FROM events e \
                     WHERE e.kind='steering_status' AND json_extract(e.payload,'$.state')='pending')",
                    [],
                    |row| row.get(0),
                )
                .expect("the search startup used to run");
            eprintln!(
                "the search startup used to run over every event: {:?} (found {pending})",
                scan.elapsed()
            );
            *mark.lock().expect("mark") = std::time::Instant::now();
        });
        let store = &fixture.store;
        let started = *begun.lock().expect("begun");
        let report = store.recovered().await;
        let recovered = started.elapsed();
        let chats = store.chats().await.expect("chats").len();
        let listed = started.elapsed();
        eprintln!("first start: recovered in {recovered:?}, {chats} chats listed after {listed:?}");
        assert_eq!(report.interrupted_turns, 3);
        assert_eq!(chats, 43);
        assert!(
            listed < Duration::from_secs(1),
            "the first list waited {listed:?}"
        );
        // The pass after startup builds the indexes and compacts the large turn it was left.
        let pass = std::time::Instant::now();
        for _ in 0..2_400 {
            let left = on_store(store, |conn| {
                let turns: i64 = conn
                    .query_row(
                        "SELECT count(*) FROM turns WHERE status <> 'running' AND compacted_at IS NULL",
                        [],
                        |row| row.get(0),
                    )
                    .expect("turns");
                let indexes: i64 = conn
                    .query_row(
                        "SELECT count(*) FROM sqlite_schema WHERE type = 'index' \
                         AND name IN ('events_steering', 'events_turn_ended')",
                        [],
                        |row| row.get(0),
                    )
                    .expect("indexes");
                (turns, indexes)
            })
            .await;
            if left == (0, 2) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        eprintln!("the pass after startup finished in {:?}", pass.elapsed());
        assert!(store.shutdown(Duration::from_secs(30)).await);

        // The next start, with a turn left running again.
        let profile = fixture.profile.clone();
        let owner = ProfileOwner::acquire(&profile).expect("owner");
        let (reopened, _) = Store::open(&profile).expect("reopen");
        reopened
            .conn()
            .execute_batch(
                "DELETE FROM events WHERE turn_id = 'live-1' AND kind = 'turn_ended'; \
                 UPDATE turns SET status = 'running', stop_reason = NULL, ended_at = NULL, \
                 compacted_at = NULL WHERE id = 'live-1';",
            )
            .expect("leave a turn running");
        let again = ChatStore::start_with(
            reopened,
            owner,
            &profile,
            "dev".into(),
            Handle::current(),
            config,
        )
        .expect("start");
        let started = std::time::Instant::now();
        let report = again.recovered().await;
        let recovered = started.elapsed();
        let chats = again.chats().await.expect("chats").len();
        let listed = started.elapsed();
        eprintln!("next start: recovered in {recovered:?}, {chats} chats listed after {listed:?}");
        assert_eq!(report.interrupted_turns, 1);
        assert!(
            listed < Duration::from_secs(1),
            "the next list waited {listed:?}"
        );
        assert!(again.shutdown(Duration::from_secs(30)).await);
    }

    /// Marks the child copy of the crash tests; holds the data root it works on.
    const CRASH_CHILD: &str = "O7_CORE_TEST_CRASH_CHILD_ROOT";

    /// Runs this test binary again as a child that does `step` on the profile in `root` and then
    /// aborts mid-transaction, the way a crash or a power cut would stop it.
    fn crash_in_child(test: &str, root: &Path) {
        let status = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args(["--exact", test, "--test-threads=1", "--nocapture"])
            .env(CRASH_CHILD, root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("run the child");
        assert!(!status.success(), "the child must die mid-transaction");
    }

    /// S40: removing the queued prompt and starting its turn are one transaction, so after a crash
    /// in the middle the prompt is still queued and no turn exists.
    #[test]
    fn begin_turn_is_atomic() {
        if let Some(root) = std::env::var_os(CRASH_CHILD) {
            let profile = profile(Path::new(&root));
            let (mut store, _) = Store::open(&profile).expect("store");
            let chat: String = store
                .conn()
                .query_row("SELECT id FROM chats", [], |row| row.get(0))
                .expect("chat");
            let tx = store.conn_mut().transaction().expect("tx");
            rows::begin_turn(
                &tx,
                "dev",
                &TurnRow {
                    continuation_prefix: None,
                    checkpoint_ref: None,
                    parts: Vec::new(),
                    id: "t1".into(),
                    chat_id: chat.clone(),
                    agent_session_id: None,
                    ordinal: 1,
                    prompt_id: "p1".into(),
                    first_event_seq: 0,
                    started_at: 1,
                },
                &[event_row(
                    0,
                    Some("t1"),
                    &O7Event::TurnStarted {
                        turn_id: "t1".into(),
                        prompt_id: Some("p1".into()),
                    },
                )],
            )
            .expect("begin");
            // Both statements ran; the process dies before the commit.
            std::process::abort();
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let profile = profile(dir.path());
        {
            let (store, _) = Store::open(&profile).expect("store");
            let chat = rows::create_chat(
                store.conn(),
                &NewChat {
                    agent_host: Default::default(),
                    agent_id: "mock".into(),
                    folder: PathBuf::from("/w"),
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
                    id: "p1".into(),
                    chat_id: chat.id,
                    text: "hi".into(),
                    held: false,
                    created_at: 1,
                },
            )
            .expect("queue");
        }
        crash_in_child("chatstore::tests::begin_turn_is_atomic", dir.path());
        let (store, _) = Store::open(&profile).expect("reopen");
        let queued: i64 = store
            .conn()
            .query_row("SELECT count(*) FROM queued_prompts", [], |row| row.get(0))
            .expect("queued");
        let turns: i64 = store
            .conn()
            .query_row("SELECT count(*) FROM turns", [], |row| row.get(0))
            .expect("turns");
        let events: i64 = store
            .conn()
            .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
            .expect("events");
        assert_eq!((queued, turns, events), (1, 0, 0));
    }

    /// S40, the other half: a `BeginTurn` that commits through the writer removes the queued prompt
    /// it started, so a started prompt is never queued as well (a restart would bring it back held,
    /// and Send queued would send it twice); the other queued prompts stay.
    #[tokio::test]
    async fn begin_turn_removes_its_queued_prompt() {
        let fixture = start(WriterConfig::STANDARD);
        let store = &fixture.store;
        let chat = new_chat(store).await;
        for (id, created_at) in [("p1", 1), ("p2", 2)] {
            send(
                store,
                Op::QueuePut(QueuedRow {
                    parts: Vec::new(),
                    id: id.into(),
                    chat_id: chat.id.clone(),
                    text: format!("prompt {id}"),
                    held: false,
                    created_at,
                }),
            )
            .await;
        }
        send(
            store,
            Op::BeginTurn {
                turn: TurnRow {
                    continuation_prefix: None,
                    checkpoint_ref: None,
                    parts: Vec::new(),
                    id: "t1".into(),
                    chat_id: chat.id.clone(),
                    agent_session_id: None,
                    ordinal: 1,
                    prompt_id: "p1".into(),
                    first_event_seq: 0,
                    started_at: 3,
                },
                rows: vec![event_row(
                    0,
                    Some("t1"),
                    &O7Event::TurnStarted {
                        turn_id: "t1".into(),
                        prompt_id: Some("p1".into()),
                    },
                )],
            },
        )
        .await;
        assert!(store.barrier().await);
        let (stored, read) = std::sync::mpsc::channel();
        let chat_id = chat.id.clone();
        store.with_store(move |store| {
            let queued: Vec<String> = rows::queued(store.conn(), &chat_id)
                .expect("queued")
                .into_iter()
                .map(|row| row.id)
                .collect();
            let turns: i64 = store
                .conn()
                .query_row("SELECT count(*) FROM turns", [], |row| row.get(0))
                .expect("turns");
            let _ = stored.send((queued, turns));
        });
        assert_eq!(
            read.recv_timeout(Duration::from_secs(10)),
            Ok((vec!["p2".to_owned()], 1))
        );
    }

    /// S2: a crash in the middle of compaction rolls back, and the next start compacts the turn.
    #[test]
    fn compaction_is_idempotent() {
        if let Some(root) = std::env::var_os(CRASH_CHILD) {
            let profile = profile(Path::new(&root));
            let (mut store, _) = Store::open(&profile).expect("store");
            let chat: String = store
                .conn()
                .query_row("SELECT id FROM chats", [], |row| row.get(0))
                .expect("chat");
            let tx = store.conn_mut().transaction().expect("tx");
            assert!(
                compact::apply(
                    &tx,
                    &chat,
                    "t1",
                    "dev",
                    &compact::CompactLimits::STANDARD,
                    9
                )
                .expect("compact")
            );
            std::process::abort();
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let profile = profile(dir.path());
        let chat = {
            let (store, _) = Store::open(&profile).expect("store");
            let chat = rows::create_chat(
                store.conn(),
                &NewChat {
                    agent_host: Default::default(),
                    agent_id: "mock".into(),
                    folder: PathBuf::from("/w"),
                    device_id: "dev".into(),
                    now: 1,
                },
            )
            .expect("chat")
            .id;
            store
                .conn()
                .execute(
                    "INSERT INTO turns (id, chat_id, seq, status, started_at, device_id, first_event_seq) \
                     VALUES ('t1', ?1, 1, 'completed', 1, 'dev', 0)",
                    params![chat],
                )
                .expect("turn");
            let delta = |text: &str| O7Event::MessageDelta {
                message_id: "t1.m1".into(),
                text: text.into(),
            };
            rows::append_events(
                store.conn(),
                "dev",
                &chat,
                &[
                    event_row(0, Some("t1"), &delta("one ")),
                    event_row(1, Some("t1"), &delta("two")),
                ],
            )
            .expect("events");
            chat
        };
        crash_in_child("chatstore::tests::compaction_is_idempotent", dir.path());
        let (mut store, _) = Store::open(&profile).expect("reopen");
        let rows_left: i64 = store
            .conn()
            .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
            .expect("rows");
        assert_eq!(rows_left, 2, "rolled back");
        let report = recover::recover(
            store.conn_mut(),
            recover::Scope::All,
            "dev",
            &compact::CompactLimits::STANDARD,
            10,
        )
        .expect("recover");
        assert_eq!(report.compacted_turns, 1);
        let text: String = store
            .conn()
            .query_row(
                "SELECT payload FROM events WHERE chat_id = ?1",
                params![chat],
                |row| row.get(0),
            )
            .expect("merged");
        assert!(text.contains("one two"), "{text}");
    }

    #[test]
    fn store_errors_read_as_sentences() {
        assert_eq!(
            StoreError::DiskFull.to_string(),
            "The disk is full, so o7 paused saving. Free some space, then try again."
        );
        assert_eq!(
            StoreError::BlobMissing(
                "The full output isn't saved yet because the disk is full.".into()
            )
            .to_string(),
            "The full output isn't saved yet because the disk is full."
        );
        assert_eq!(
            serde_json::to_value(TurnStatus::Interrupted).expect("serialize"),
            serde_json::json!("interrupted")
        );
        assert_eq!(
            serde_json::to_value(TitleSource::FirstLine).expect("serialize"),
            serde_json::json!("first_line")
        );
    }
}
