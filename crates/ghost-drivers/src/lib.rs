//! Agent drivers: everything that turns an agent's native stream into [`GhostEvent`]s.
//!
//! - [`driver`]: the contract between a session and a driver (the actor, its commands and events,
//!   the bounded event channel, the spawner seam). Frozen for M1a; `testing` (feature `testing`) holds its test
//!   doubles for the session manager's tests.
//! - [`batch`]: the [`batch::FrameBatcher`], which packs events into size-limited, sequence-numbered
//!   frames for the UI, carrying each event's turn tag for storage.
//! - [`mock`]: a scripted agent timeline for the UI demo, the streaming stress test and the demo
//!   agent's driver ([`mock_driver`]).
//! - [`jsonrpc`] and [`acp`]: our own JSON-RPC peer and the Agent Client Protocol driver, with one
//!   dialect per agent family; [`factory`] creates the driver for each kind.
//! - `replay` (feature `replay`): the ACP driver against a fake agent playing a recorded protocol
//!   log, compared with golden events.
//!
//! [`GhostEvent`]: ghost_protocol::GhostEvent

pub mod acp;
pub mod attachments;
pub mod batch;
pub mod claude;
pub mod codex;
pub mod driver;
pub mod elicitation;
pub mod factory;
pub mod headless;
pub mod jsonrpc;
pub mod mcp;
pub mod mock;
pub mod mock_driver;
pub mod pi;
#[cfg(feature = "replay")]
pub mod replay;
pub mod terminal;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub mod controls;
pub mod incoming_images;

/// Preserve a native image's file reference without transporting its binary payload as text.
/// The UI accepts local paths only and the session validates the file against its chat root.
pub(crate) fn image_reference(label: &str, path: &str) -> String {
    if path.is_empty() || path.len() > 4096 || path.contains(['<', '>', '\n', '\r', '\0']) {
        return "Image reference unavailable.".into();
    }
    let label = label.replace(['[', ']', '\n', '\r'], " ");
    format!("![{label}](<{path}>)")
}
