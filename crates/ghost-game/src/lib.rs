//! Games for Ghost (docs/game.md): what a project's engine is, Godot found or fetched and run out
//! of sight, the bridge to the running game, and the tools agents play it with.
//!
//! No Tauri here: the desktop app holds a [`Manager`], shows its [`GameStatus`] in the Game pane
//! and passes agents' tool calls to [`Manager::tool`].
//!
//! Who may do what is held in this crate, not in the pane:
//! - Fetching Godot is the person's ([`Manager::fetch_start`]); no tool starts it.
//! - A chat runs the game of its own folder only: the folder comes from the caller that owns the
//!   chat (the session's grant, the pane's command), never from a tool's arguments.
//! - The running game is not trusted. It connects to a loopback listener of this run, must show
//!   the run's token before anything else, and from then on can only answer and send pictures:
//!   every message is bounded, and what it says is data for the agent, never an instruction.

pub mod assets;
pub mod bridge;
mod check;
pub mod detect;
pub mod godot;
mod manager;
mod output;
pub mod probe;
mod run;
pub mod starter;
pub mod tools;

pub use detect::{GameEngine, GameProject, detect};
pub use manager::{
    ChatGame, Config, DEFAULT_RUNNING, GameError, GameInput, GameLog, GamePicked, GameRecent,
    GameState, GameStatus, GameSummon, GameTunable, GameTunables, GodotFetch, GodotStatus, IDLE,
    Manager, TURNED_OFF, ToolReply, Viewer, WebFuture, WebGames,
};
pub use probe::EngineProbe;
pub use starter::Starter;

use std::path::Path;

/// Why something with a game did not work, in words for the person or the agent.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// It is not allowed, or cannot be done as asked.
    #[error("{0}")]
    Refused(String),
    /// The engine or the game failed, in its own words where it gave any.
    #[error("{0}")]
    Game(String),
    /// The running game answered in a way the bridge does not allow, or stopped answering.
    #[error("{0}")]
    Bridge(String),
    #[error("The game took too long to answer.")]
    Timeout,
    /// Godot's download did not arrive whole and correct.
    #[error("{0}")]
    Download(String),
    #[error("Stopped.")]
    Cancelled,
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// A command for a helper program (Godot asked its version): no console window on Windows, no
/// input.
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
