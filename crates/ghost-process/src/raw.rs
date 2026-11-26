//! Bounded binary stdin for owned transports. Payloads never enter protocol or stderr logs.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch};

use crate::{MAX_LINE_BYTES, ProcessControl};

/// Pending writes normally share this byte budget. One larger frame holds the entire budget.
pub const RAW_STDIN_BYTES: usize = 8 * 1024 * 1024;
/// One native line plus its binary transport envelope, never an unbounded write.
pub const MAX_RAW_WRITE_BYTES: usize = MAX_LINE_BYTES + 16;
const RAW_STDIN_PACKETS: usize = 16;

struct Packet {
    bytes: Vec<u8>,
    _budget: OwnedSemaphorePermit,
}

/// A cloneable binary writer with whole-packet enqueue and bounded pending memory.
/// Cancelling `write` before it returns leaves no part of that packet queued. Once queued, the
/// writer owns it; a later close or kill may discard it or interrupt its operating-system write.
#[derive(Clone)]
pub struct ByteSink {
    tx: mpsc::Sender<Packet>,
    budget: Arc<Semaphore>,
    closed: Arc<AtomicBool>,
    close: watch::Sender<bool>,
}

impl std::fmt::Debug for ByteSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ByteSink")
            .field("closed", &self.is_closed())
            .finish_non_exhaustive()
    }
}

impl ByteSink {
    pub(crate) fn channel() -> (Self, ByteReceiver) {
        let (tx, rx) = mpsc::channel(RAW_STDIN_PACKETS);
        let (close, stopped) = watch::channel(false);
        let sink = Self {
            tx,
            budget: Arc::new(Semaphore::new(RAW_STDIN_BYTES)),
            closed: Arc::new(AtomicBool::new(false)),
            close,
        };
        (sink, ByteReceiver { rx, stopped })
    }

    fn wanted(&self, bytes: &[u8]) -> io::Result<u32> {
        if bytes.len() > MAX_RAW_WRITE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "raw write is too large",
            ));
        }
        if self.is_closed() {
            return Err(closed());
        }
        Ok(bytes.len().min(RAW_STDIN_BYTES) as u32)
    }

    /// Queue a complete packet, waiting for byte and packet capacity. No text conversion, newline
    /// insertion or logging is performed. A successful enqueue is not an operating-system flush.
    pub async fn write(&self, bytes: Vec<u8>) -> io::Result<()> {
        let wanted = self.wanted(&bytes)?;
        let budget = Arc::clone(&self.budget)
            .acquire_many_owned(wanted)
            .await
            .map_err(|_| closed())?;
        let slot = self.tx.reserve().await.map_err(|_| closed())?;
        if self.is_closed() {
            return Err(closed());
        }
        slot.send(Packet {
            bytes,
            _budget: budget,
        });
        Ok(())
    }

    /// Queue immediately or return WouldBlock. A rejected packet is never partly queued.
    pub fn try_write(&self, bytes: Vec<u8>) -> io::Result<()> {
        let wanted = self.wanted(&bytes)?;
        let budget = Arc::clone(&self.budget)
            .try_acquire_many_owned(wanted)
            .map_err(|error| match error {
                tokio::sync::TryAcquireError::Closed => closed(),
                tokio::sync::TryAcquireError::NoPermits => full(),
            })?;
        self.tx
            .try_send(Packet {
                bytes,
                _budget: budget,
            })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => full(),
                mpsc::error::TrySendError::Closed(_) => closed(),
            })
    }

    /// Close immediately, interrupt any blocked write and discard queued transport data.
    pub fn close(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            self.budget.close();
            self.close.send_replace(true);
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire) || self.tx.is_closed()
    }

    pub async fn writer_closed(&self) {
        self.tx.closed().await;
    }
}

fn closed() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "raw stdin is closed")
}
fn full() -> io::Error {
    io::Error::new(io::ErrorKind::WouldBlock, "raw stdin queue is full")
}

pub(crate) struct ByteReceiver {
    rx: mpsc::Receiver<Packet>,
    stopped: watch::Receiver<bool>,
}

impl ByteReceiver {
    pub(crate) async fn forward(mut self, mut writer: impl AsyncWrite + Unpin) {
        loop {
            let packet = tokio::select! {
                biased;
                _ = self.stopped.wait_for(|stop| *stop) => break,
                packet = self.rx.recv() => match packet {
                    Some(packet) => packet,
                    None => break,
                },
            };
            let result = tokio::select! {
                biased;
                _ = self.stopped.wait_for(|stop| *stop) => break,
                result = async {
                    writer.write_all(&packet.bytes).await?;
                    writer.flush().await
                } => result,
            };
            if result.is_err() {
                break;
            }
        }
        // Drop the OS pipe rather than waiting on shutdown while its peer may no longer read.
    }
}

/// An owned native process with raw pipes. The caller must drain stdout and stderr concurrently.
/// Neither stream nor stdin is copied to logs. Dropping the last control still kills its tree.
#[derive(Debug)]
pub struct RawProcess {
    pub stdin: ByteSink,
    pub stdout: tokio::process::ChildStdout,
    pub stderr: tokio::process::ChildStderr,
    pub control: ProcessControl,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn raw_bytes_are_unchanged_and_close_interrupts_a_blocked_writer() {
        let (sink, receiver) = ByteSink::channel();
        let (writer, mut reader) = tokio::io::duplex(4);
        let task = tokio::spawn(receiver.forward(writer));
        sink.write(vec![0, 255, 10, 13]).await.unwrap();
        let mut bytes = [0; 4];
        reader.read_exact(&mut bytes).await.unwrap();
        assert_eq!(bytes, [0, 255, 10, 13]);
        sink.write(vec![42; 100]).await.unwrap();
        sink.close();
        tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            sink.write(vec![1]).await.unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
    }

    #[tokio::test]
    async fn cancelled_write_never_enqueues_a_partial_packet() {
        let (sink, mut receiver) = ByteSink::channel();
        sink.try_write(vec![1; RAW_STDIN_BYTES]).unwrap();
        assert_eq!(
            sink.try_write(vec![2]).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(1), sink.write(vec![3; 10]))
                .await
                .is_err()
        );
        drop(receiver.rx.recv().await.unwrap());
        assert!(receiver.rx.try_recv().is_err());
        sink.try_write(vec![4]).unwrap();
        assert_eq!(receiver.rx.recv().await.unwrap().bytes, [4]);
        sink.close();
        assert_eq!(
            sink.try_write(vec![5]).unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
    }
}
