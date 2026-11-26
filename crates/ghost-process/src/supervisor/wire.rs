//! Versioned, bounded control framing for an owned Linux supervisor. Agent lines are raw UTF-8
//! payloads, never embedded in JSON: a large escaped line cannot multiply the allocation limit.

use std::io;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::environment::LaunchEnvironment;
use crate::{DaemonMatch, ExitInfo, MAX_LINE_BYTES};

pub const PROTOCOL_VERSION: u32 = 2;
const CONTROL_LIMIT: usize = 1024 * 1024;

/// The parent explicitly requests one process. Environment values stay on the Linux host: the
/// supervisor resolves its login environment locally rather than exporting credentials to Windows.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchRequest {
    pub protocol: u32,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
    /// An inspection on this same leased connection authorizes this launch's host realm.
    #[serde(default)]
    pub preparation: Option<String>,
    #[serde(default)]
    pub set_env: LaunchEnvironment,
    pub daemons: Vec<DaemonMatch>,
    pub max_line_bytes: usize,
}

impl LaunchRequest {
    pub fn validate(&self) -> io::Result<()> {
        self.set_env.validate()?;
        let plain = |value: &str| !value.contains('\0');
        if self.protocol != PROTOCOL_VERSION
            || !self.program.starts_with('/')
            || !self.cwd.starts_with('/')
            || !plain(&self.program)
            || !plain(&self.cwd)
            || self
                .preparation
                .as_ref()
                .is_some_and(|id| uuid::Uuid::parse_str(id).is_err())
            || self.args.len() > 1024
            || self.args.iter().any(|arg| !plain(arg))
            || self.daemons.len() > 16
            || self.daemons.iter().any(|daemon| {
                (daemon.program.is_none() && daemon.args_in_order.is_empty())
                    || daemon
                        .program
                        .as_ref()
                        .is_some_and(|name| name.is_empty() || !plain(name))
                    || daemon.args_in_order.len() > 32
                    || daemon
                        .args_in_order
                        .iter()
                        .any(|arg| arg.is_empty() || !plain(arg))
            })
            || self.max_line_bytes == 0
            || self.max_line_bytes > MAX_LINE_BYTES
        {
            return Err(invalid("invalid supervisor launch request"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ready {
    pub protocol: u32,
    pub pid: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Oversized {
    pub bytes: u64,
    pub id: Option<serde_json::Value>,
    pub method: Option<String>,
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ended {
    pub exit: Option<ExitInfo>,
    pub leftovers: Vec<u32>,
    pub spared: Vec<u32>,
    /// Redacted on the Linux host before it crosses the transport boundary.
    pub stderr_tail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Tag {
    Launch = 1,
    Stdin = 2,
    CloseStdin = 3,
    Heartbeat = 4,
    Stop = 5,
    Inspect = 6,
    Ready = 16,
    Stdout = 17,
    Oversized = 18,
    Ended = 19,
    Failure = 20,
    Inspection = 21,
}

impl Tag {
    fn parse(tag: u8) -> io::Result<Self> {
        match tag {
            1 => Ok(Self::Launch),
            2 => Ok(Self::Stdin),
            3 => Ok(Self::CloseStdin),
            4 => Ok(Self::Heartbeat),
            5 => Ok(Self::Stop),
            6 => Ok(Self::Inspect),
            16 => Ok(Self::Ready),
            17 => Ok(Self::Stdout),
            18 => Ok(Self::Oversized),
            19 => Ok(Self::Ended),
            20 => Ok(Self::Failure),
            21 => Ok(Self::Inspection),
            _ => Err(invalid("unknown supervisor frame tag")),
        }
    }

    fn limit(self) -> usize {
        match self {
            Self::Stdin | Self::Stdout => MAX_LINE_BYTES,
            Self::CloseStdin | Self::Heartbeat | Self::Stop => 0,
            _ => CONTROL_LIMIT,
        }
    }
}

/// Contains process protocol text. Deliberately has no Debug implementation.
pub struct Frame {
    pub tag: Tag,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn empty(tag: Tag) -> Self {
        Self {
            tag,
            payload: Vec::new(),
        }
    }

    pub fn json<T: Serialize>(tag: Tag, value: &T) -> io::Result<Self> {
        let payload = serde_json::to_vec(value).map_err(io::Error::other)?;
        if payload.len() > tag.limit() {
            return Err(invalid("supervisor frame exceeds its limit"));
        }
        Ok(Self { tag, payload })
    }

    pub fn decode<T: serde::de::DeserializeOwned>(&self) -> io::Result<T> {
        // Do not propagate serde's error text: it may quote protocol or environment values.
        serde_json::from_slice(&self.payload)
            .map_err(|_| invalid("invalid supervisor control frame"))
    }
}

/// Cancellation must discard the stream. The server's single reader task owns partial frames.
pub async fn read_frame(reader: &mut (impl AsyncRead + Unpin)) -> io::Result<Option<Frame>> {
    let mut tag = [0];
    if reader.read(&mut tag).await? == 0 {
        return Ok(None);
    }
    let tag = Tag::parse(tag[0])?;
    let length = reader.read_u32().await? as usize;
    if length > tag.limit() {
        return Err(invalid("supervisor frame exceeds its limit"));
    }
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload).await?;
    Ok(Some(Frame { tag, payload }))
}

pub async fn write_frame(writer: &mut (impl AsyncWrite + Unpin), frame: &Frame) -> io::Result<()> {
    if frame.payload.len() > frame.tag.limit() {
        return Err(invalid("supervisor frame exceeds its limit"));
    }
    writer.write_u8(frame.tag as u8).await?;
    writer
        .write_u32(u32::try_from(frame.payload.len()).map_err(io::Error::other)?)
        .await?;
    writer.write_all(&frame.payload).await?;
    writer.flush().await
}

pub(crate) fn invalid(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}

/// A parent that has vanished cannot keep its Linux process alive through an orphaned relay.
pub const LEASE: Duration = Duration::from_secs(15);
