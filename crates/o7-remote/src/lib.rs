//! Remote sessions: a paired browser or a linked bot reaching the chats on this desktop.
//!
//! The desktop keeps one outbound WebSocket to a relay and opens no port. A browser pairs once
//! with a link, then every message between it and the desktop is sealed end to end
//! ([`crypto`]); the relay forwards what it cannot read. A bot speaks MCP to the relay, which
//! turns each tool call into one request here ([`bot`]). See `docs/remote-sessions.md`.

pub mod backend;
pub mod bot;
pub mod callback;
pub mod crypto;
pub mod host;
pub mod registry;
pub mod relay;
pub mod render;
pub mod wire;

pub use backend::{
    Action, Backend, BackendError, BoxFuture, BranchChanges, ChatFacts, ChatOptions, Choice, Diff,
    Frames, ImagePreview, Outcome, Picture, PictureOf, PrChecks, PrNote, PreparedImage,
    PullRequest, Statuses, TaskChat,
};
pub use callback::{Callback, CallbackEvent, CallbackKind, Delivery};
pub use host::{Config, Service};
pub use registry::RegistryError;
pub use wire::{Look, Overview, RemoteAgent};
