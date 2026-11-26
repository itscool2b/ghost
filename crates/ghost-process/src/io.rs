//! An agent's pipes as the driver sees them.
//!
//! stdin is a [`LineSink`] that never blocks: a writer task drains an unbounded queue, writes
//! `line\n` and flushes per line, so a full stdin pipe can never stall the driver's stdout reading.
//! stdout is a [`LineSource`] bounded at [`STDOUT_CHANNEL_LINES`] lines and [`STDOUT_CHANNEL_BYTES`]
//! bytes (one longer line passes alone): a slow driver stops the reader, the pipe fills, and the
//! agent blocks on its own write, so memory stays bounded however fast the agent writes.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};

use crate::framing::{Frame, LineFramer};
use crate::log::{Direction, LogNote, ProtocolLog};
use crate::{STDOUT_CHANNEL_BYTES, STDOUT_CHANNEL_LINES};

/// stdout is read in chunks of this size.
const READ_CHUNK: usize = 64 * 1024;

/// What an [`StdoutItem::Oversized`] item reserves of the byte budget: it carries no line.
const OVERSIZED_ITEM_BYTES: usize = 256;

/// The agent's pipes.
#[derive(Debug)]
pub struct AgentIo {
    pub stdin: LineSink,
    pub stdout: LineSource,
}

/// What the stdin writer task receives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StdinMsg {
    Line(String),
    /// End of file after the lines queued before it.
    Close,
}

/// Lines to the agent's stdin. Cloning shares the queue.
#[derive(Debug, Clone)]
pub struct LineSink {
    tx: mpsc::UnboundedSender<StdinMsg>,
    closed: Arc<AtomicBool>,
}

impl LineSink {
    /// A sink and the receiver its writer task drains.
    pub(crate) fn channel() -> (LineSink, mpsc::UnboundedReceiver<StdinMsg>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let sink = LineSink {
            tx,
            closed: Arc::new(AtomicBool::new(false)),
        };
        (sink, rx)
    }

    /// Queues one line (no trailing newline). Never blocks: a writer task writes `line\n` and
    /// flushes. Fails once stdin was closed or the writer stopped (the agent is gone).
    pub fn send(&self, line: String) -> Result<(), StdinClosed> {
        if self.closed.load(Ordering::Acquire) {
            return Err(StdinClosed);
        }
        self.tx.send(StdinMsg::Line(line)).map_err(|_| StdinClosed)
    }

    /// End of file after the queued lines. Later sends fail; calling it again does nothing.
    pub fn close(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            // The writer may be gone already, which closes stdin just the same.
            let _ = self.tx.send(StdinMsg::Close);
        }
    }

    /// Whether `close` was called on this sink or one of its clones.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire) || self.tx.is_closed()
    }

    /// Wait until the writer has stopped receiving queued lines, including asynchronous EPIPE
    /// after `send` succeeded. Cancel-safe for a driver's select loop. After an intentional close,
    /// this resolves when the writer drains its queue and exits, not when `close` is called.
    pub async fn writer_closed(&self) {
        self.tx.closed().await;
    }
}

/// The agent's stdin no longer takes lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the agent's stdin is closed")]
pub struct StdinClosed;

/// Lines from the agent's stdout, bounded by count and bytes.
#[derive(Debug)]
pub struct LineSource {
    rx: mpsc::Receiver<(StdoutItem, OwnedSemaphorePermit)>,
    bytes: Arc<Semaphore>,
}

impl LineSource {
    /// The next line, or `None` at end of file (or a read error, which ends the stream the same
    /// way). Cancel-safe: use it in `select!`.
    pub async fn next(&mut self) -> Option<StdoutItem> {
        // The permit releases the line's bytes as it drops here.
        self.rx.recv().await.map(|(item, _permit)| item)
    }
}

impl Drop for LineSource {
    fn drop(&mut self) {
        // Wakes a reader waiting for byte budget, so it sees the closed channel and stops.
        self.bytes.close();
    }
}

/// The reader's end of a [`LineSource`].
#[derive(Debug)]
pub(crate) struct StdoutSender {
    tx: mpsc::Sender<(StdoutItem, OwnedSemaphorePermit)>,
    bytes: Arc<Semaphore>,
}

/// The driver dropped its [`LineSource`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SourceGone;

/// A bounded stdout channel: at most [`STDOUT_CHANNEL_LINES`] items and [`STDOUT_CHANNEL_BYTES`]
/// bytes in flight.
pub(crate) fn stdout_channel() -> (StdoutSender, LineSource) {
    let (tx, rx) = mpsc::channel(STDOUT_CHANNEL_LINES);
    let bytes = Arc::new(Semaphore::new(STDOUT_CHANNEL_BYTES));
    let sender = StdoutSender {
        tx,
        bytes: Arc::clone(&bytes),
    };
    (sender, LineSource { rx, bytes })
}

impl StdoutSender {
    /// Waits until the item fits, then queues it. An item larger than the whole byte budget waits
    /// for all of it and so passes alone.
    pub(crate) async fn send(&self, item: StdoutItem) -> Result<(), SourceGone> {
        let size = match &item {
            StdoutItem::Line(line) => line.len(),
            StdoutItem::Oversized { .. } => OVERSIZED_ITEM_BYTES,
        };
        let wanted = u32::try_from(size.min(STDOUT_CHANNEL_BYTES)).unwrap_or(u32::MAX);
        let permit = Arc::clone(&self.bytes)
            .acquire_many_owned(wanted)
            .await
            .map_err(|_| SourceGone)?;
        self.tx.send((item, permit)).await.map_err(|_| SourceGone)
    }
}

/// The stdin writer task: writes each queued line with its newline and flushes, logging it `out`.
/// A write error stops it (the agent is gone), and so does `close`; either way stdin closes and
/// later sends fail.
pub(crate) async fn write_stdin(
    mut pipe: impl AsyncWrite + Unpin,
    mut lines: mpsc::UnboundedReceiver<StdinMsg>,
    log: ProtocolLog,
) {
    while let Some(message) = lines.recv().await {
        let StdinMsg::Line(line) = message else {
            break;
        };
        log.line(Direction::Out, &line);
        let mut bytes = line.into_bytes();
        bytes.push(b'\n');
        if pipe.write_all(&bytes).await.is_err() || pipe.flush().await.is_err() {
            break;
        }
    }
    let _ = pipe.shutdown().await;
}

/// The stdout reader task: frames the pipe, logs every line `in` (noise and oversized lines as
/// notes), and hands lines to the driver through the bounded channel, so a slow driver stops the
/// reading. A read error ends the stream like end of file. If the driver drops its source, the pipe
/// is still read to its end and logged, so the agent never blocks on a pipe nobody reads.
pub(crate) async fn read_stdout(
    mut pipe: impl AsyncRead + Unpin,
    sender: StdoutSender,
    log: ProtocolLog,
    max_line_bytes: usize,
) {
    let mut framer = LineFramer::new(max_line_bytes);
    let mut chunk = vec![0u8; READ_CHUNK];
    let mut sender = Some(sender);
    loop {
        let read = match pipe.read(&mut chunk).await {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        for frame in framer.push(chunk.get(..read).unwrap_or_default()) {
            deliver(frame, &mut sender, &log).await;
        }
    }
    if let Some(frame) = framer.finish() {
        deliver(frame, &mut sender, &log).await;
    }
}

async fn deliver(frame: Frame, sender: &mut Option<StdoutSender>, log: &ProtocolLog) {
    let item = match frame {
        Frame::Line(line) => {
            log.line(Direction::In, &line);
            StdoutItem::Line(line)
        }
        Frame::Oversized { bytes, keys } => {
            log.note(LogNote::Oversized {
                bytes,
                method: keys.method.clone(),
            });
            StdoutItem::Oversized { bytes, keys }
        }
        Frame::Noise { bytes } => {
            log.note(LogNote::Noise { bytes });
            return;
        }
    };
    if let Some(open) = sender
        && open.send(item).await.is_err()
    {
        *sender = None;
    }
}

/// One item from stdout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StdoutItem {
    /// A JSON line (its first non-space byte is `{` or `[`), lossily decoded, without its line
    /// ending.
    Line(String),
    /// A line over the cap: skipped, with the keys the sniffer found in it.
    Oversized { bytes: u64, keys: TopKeys },
}

/// The top-level keys of an oversized line, found in constant memory wherever they appear.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TopKeys {
    /// The raw top-level `"id"` value.
    pub id: Option<serde_json::Value>,
    pub method: Option<String>,
    /// The top-level `"type"`.
    pub kind: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_sink_refuses_lines_and_sends_one_close() {
        let (sink, mut rx) = LineSink::channel();
        let clone = sink.clone();
        sink.send("one".into()).expect("open");
        clone.close();
        clone.close();
        assert!(sink.is_closed());
        assert_eq!(sink.send("two".into()), Err(StdinClosed));
        assert_eq!(rx.try_recv().ok(), Some(StdinMsg::Line("one".into())));
        assert_eq!(rx.try_recv().ok(), Some(StdinMsg::Close));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn a_sink_whose_writer_stopped_refuses_lines() {
        let (sink, rx) = LineSink::channel();
        drop(rx);
        assert_eq!(sink.send("x".into()), Err(StdinClosed));
        assert!(sink.is_closed());
    }

    #[tokio::test(start_paused = true)]
    async fn a_writer_failure_wakes_waiters_after_a_cancelled_wait() {
        let (sink, rx) = LineSink::channel();
        sink.send("queued before failure".into()).unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(1), sink.writer_closed())
                .await
                .is_err()
        );
        drop(rx);
        tokio::time::timeout(std::time::Duration::from_millis(1), sink.writer_closed())
            .await
            .unwrap();
        assert!(sink.is_closed());
    }

    #[tokio::test]
    async fn stdout_items_arrive_in_order_and_end_with_the_sender() {
        let (sender, mut source) = stdout_channel();
        sender
            .send(StdoutItem::Line("{}".into()))
            .await
            .expect("send");
        sender
            .send(StdoutItem::Oversized {
                bytes: 70_000_000,
                keys: TopKeys {
                    id: Some(serde_json::json!(7)),
                    ..TopKeys::default()
                },
            })
            .await
            .expect("send");
        drop(sender);
        assert_eq!(source.next().await, Some(StdoutItem::Line("{}".into())));
        assert!(matches!(
            source.next().await,
            Some(StdoutItem::Oversized {
                bytes: 70_000_000,
                ..
            })
        ));
        assert_eq!(source.next().await, None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_reader_bounds_what_is_in_flight() {
        let (sender, mut source) = stdout_channel();
        // Sixteen lines fill the channel; the seventeenth waits for the reader.
        for index in 0..STDOUT_CHANNEL_LINES {
            sender
                .send(StdoutItem::Line(format!("[{index}]")))
                .await
                .expect("fits");
        }
        let blocked = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            sender.send(StdoutItem::Line("[16]".into())),
        )
        .await;
        assert!(blocked.is_err(), "the seventeenth line waits");
        assert_eq!(source.next().await, Some(StdoutItem::Line("[0]".into())));
        sender
            .send(StdoutItem::Line("[16]".into()))
            .await
            .expect("room now");

        // A line larger than the byte budget passes alone once everything else is read.
        let (sender, mut source) = stdout_channel();
        let huge = "x".repeat(STDOUT_CHANNEL_BYTES + 10);
        sender
            .send(StdoutItem::Line("[a]".into()))
            .await
            .expect("small");
        let pending = tokio::spawn(async move {
            sender
                .send(StdoutItem::Line(huge))
                .await
                .expect("passes alone");
            sender
        });
        assert_eq!(source.next().await, Some(StdoutItem::Line("[a]".into())));
        let sender = pending.await.expect("join");
        assert!(
            matches!(source.next().await, Some(StdoutItem::Line(line)) if line.len() > STDOUT_CHANNEL_BYTES)
        );
        drop(sender);
        assert_eq!(source.next().await, None);
    }

    #[tokio::test]
    async fn a_dropped_source_stops_a_waiting_sender() {
        let (sender, source) = stdout_channel();
        drop(source);
        assert_eq!(
            sender.send(StdoutItem::Line("{}".into())).await,
            Err(SourceGone)
        );
    }
}
