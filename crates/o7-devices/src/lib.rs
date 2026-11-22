//! Phones for o7 (docs/superpowers/plans/2026-11-21-mobile.md): everything between the app and a
//! device, with no Tauri in it, so it is tested as plain Rust.
//!
//! - [`adb`]: adb's wire protocol to the adb server on this computer.
//! - [`shell`]: quoting and checking words that go into a device's shell.
//! - [`scrcpy`]: a device's screen as video and its input, through the pinned scrcpy server.
//! - [`outline`]: the screen as an outline with refs, from Android's accessibility dump.
//! - [`android`]: installing, starting and reading an app; [`picture`]: screenshots for agents.
//! - [`sdk`], [`repo`], [`install`]: finding Android's tools, Google's package index, and
//!   fetching the three packages the pane needs, each checked before it is unpacked.
//! - [`avd`], [`emulator`]: virtual devices as files, and the emulator's life.
//! - [`manager`]: which chat holds which device; [`tools`]: what agents may do on theirs.
//!
//! Nothing in this crate listens on a port, and every connection it makes is to the loopback
//! address or to `dl.google.com`.

pub mod adb;
pub mod android;
pub mod avd;
pub mod emulator;
pub mod install;
pub mod manager;
pub mod outline;
pub mod picture;
pub mod repo;
pub mod scrcpy;
pub mod sdk;
pub mod shell;
pub mod tools;

use std::path::Path;

/// What went wrong, in words a person can act on. These reach the pane and agents' tool replies
/// as written.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The adb server is not there and could not be started.
    #[error("adb is not reachable: {0}")]
    AdbUnreachable(String),
    /// The adb server refused a request, in its own words.
    #[error("adb refused: {0}")]
    Adb(String),
    /// The device is not connected any more (unplugged, shut down, still booting).
    #[error("The device is not connected.")]
    DeviceGone,
    /// The phone has not allowed this computer to debug it.
    #[error("The phone has not allowed this computer yet. Unlock it and accept \"Allow USB debugging\".")]
    Unauthorized,
    /// The device answered, and the answer was a failure.
    #[error("{0}")]
    Device(String),
    /// The request was not sent because it is not allowed or not well formed.
    #[error("{0}")]
    Refused(String),
    /// Something answered in a way the protocol does not allow.
    #[error("{0}")]
    Protocol(String),
    #[error("The device took too long to answer.")]
    Timeout,
    #[error("The device sent more than o7 accepts.")]
    TooLarge,
    /// A download from Google did not arrive whole and correct.
    #[error("{0}")]
    Download(String),
    #[error("Stopped.")]
    Cancelled,
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// A command for a helper program (adb, the emulator): no console window on Windows, no input.
pub(crate) fn command(program: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(program);
    command.stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}
