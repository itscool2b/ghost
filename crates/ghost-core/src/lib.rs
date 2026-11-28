//! The local foundation of Ghost: which device this is, where its data lives, which profile is
//! active, the per-profile SQLite store, and the chat sessions that run on it.
//!
//! Nothing here depends on Tauri. The desktop shell and the CLI both find the data root through
//! [`paths::DataRoot::resolve`] (the same folder as Tauri's `app_local_data_dir()`, never the roaming
//! `app_data_dir()`, or `$GHOST_DATA_DIR`), so they always agree on where a profile lives. Startup
//! goes:
//!
//! ```no_run
//! # fn main() -> ghost_core::Result<()> {
//! use ghost_core::{device::DeviceIdentity, paths::DataRoot, profile::Profiles, store::Store};
//!
//! let root = DataRoot::resolve()?;
//! let device = DeviceIdentity::load_or_create(&root)?;
//! let profile = Profiles::new(root).open_active()?;
//! let (store, info) = Store::open(&profile)?;
//! # let _ = (device, store, info);
//! # Ok(())
//! # }
//! ```
//!
//! Each step repairs what it can instead of failing: a damaged `device.json` or `state.json` is
//! moved aside (never deleted) and replaced, a damaged database is moved aside and replaced by a
//! copy of everything still readable in it, and the outcome is reported so the UI can tell the
//! user. Errors are reserved for what can't be repaired (no permission, a file that can't be read,
//! disk full, data from a newer Ghost, a database another Ghost process is using, a linked
//! data folder whose disk isn't connected), and their messages are written to be shown as they are.
//!
//! The process that runs chats also takes the profile's owner lock ([`owner::ProfileOwner`]) between
//! opening the profile and opening its store; the store then moves into the chat store's writer
//! thread ([`chatstore::ChatStore`]), and [`session::SessionManager`] runs the chats on top of it.

pub mod agent_accounts;
pub mod agent_usage;
pub mod browser_service;
pub mod chatstore;
pub mod checkpoints;
pub mod cloning;
mod conversation;
pub mod coordinator_service;
pub mod device;
pub mod error;
pub mod game_service;
pub mod gitworkspaces;
pub mod ids;
pub mod launch_cache;
pub mod mcp;
pub mod memory_service;
pub mod mobile_service;
pub mod onboarding;
pub mod owner;
pub mod paths;
pub mod profile;
pub mod session;
pub mod store;
pub mod terminal;
pub mod time;
pub mod uninstall;

mod fsutil;
mod keyed_lock;

pub use error::{CoreError, InUseReason, LinkProblem, Result};

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use specta::Types;
    use specta::datatype::DataType;

    /// Leaves types as they are. Enough for the exporter's own checks (notably its refusal of `i64`);
    /// the real bindings go through tauri-specta, which applies specta-serde's format (renames, tags).
    struct AsIs;

    impl specta::Format for AsIs {
        fn map_types(&'_ self, types: &Types) -> Result<Cow<'_, Types>, specta::FormatError> {
            Ok(Cow::Owned(types.clone()))
        }

        fn map_type(
            &'_ self,
            _types: &Types,
            dt: &DataType,
        ) -> Result<Cow<'_, DataType>, specta::FormatError> {
            Ok(Cow::Owned(dt.clone()))
        }
    }

    /// The desktop shell generates TypeScript bindings from these types; a field specta can't export
    /// (an unmarked `i64`) would only surface there, in another crate's CI step.
    #[test]
    fn ui_facing_types_export_to_typescript() {
        let types = Types::default()
            .register::<crate::device::DeviceIdentity>()
            .register::<crate::profile::ProfileKind>()
            .register::<crate::profile::ProfileMeta>()
            .register::<crate::store::OpenOutcome>();
        let ts = specta_typescript::Typescript::default()
            .export(&types, AsIs)
            .expect("exports to TypeScript");
        for name in [
            "DeviceIdentity",
            "ProfileKind",
            "ProfileMeta",
            "OpenOutcome",
            "Salvage",
        ] {
            assert!(
                ts.contains(&format!("export type {name} =")),
                "{name} missing from:\n{ts}"
            );
        }
        assert_eq!(ts.matches("created_at: number").count(), 2, "{ts}");
    }
}
