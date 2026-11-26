//! Errors from ghost-core.
//!
//! Every message is a complete sentence a person can act on, because the desktop shell shows them in
//! the UI as they are (boot screen, profile switcher) instead of mapping each variant to its own copy.

use std::path::PathBuf;

/// Result type for everything in ghost-core.
pub type Result<T> = std::result::Result<T, CoreError>;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// A filesystem operation failed. `path` names the file or folder involved so the message points
    /// at something the user can inspect.
    #[error("Could not access {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A JSON file Ghost owns could not be parsed (or, rarely, serialized).
    #[error("{path} is not valid: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("Database error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// A database of a removed Ghost account (see `store/encrypted.rs`). It is never opened,
    /// repaired or replaced.
    #[error(
        "The database at {path} is encrypted for a Ghost account, which this version no longer opens. It was left as it is."
    )]
    EncryptedDatabase { path: PathBuf },

    /// The backup taken before a migration failed. The migration is not attempted without one.
    #[error(
        "Could not back up the database to {path} before upgrading it, so it was left unchanged: {source}"
    )]
    Backup {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },

    /// A migration failed and was rolled back, or the stored schema version is unusable.
    #[error("Could not upgrade the database, so it was left unchanged: {0}")]
    Migration(String),

    /// The database was written by a newer Ghost. Nothing was written to it.
    #[error(
        "This data was saved by a newer version of Ghost (database version {found}; this version reads up to {supported}). Update Ghost to open it."
    )]
    SchemaTooNew { found: u32, supported: u32 },

    /// The database file is damaged, as the probe, a check, or a query found. Used when the damage
    /// remains after a repair (see [`CoreError::RecoveryIncomplete`]).
    #[error("{path} is damaged: {problem}")]
    Damaged { path: PathBuf, problem: String },

    /// Damage was found, but the repair failed before the damaged file was moved (disk full while
    /// copying what was readable, no permission), so nothing changed. The next open tries again.
    #[error(
        "Ghost found damage in {path} but could not repair it, so the file was left where it is: {source}"
    )]
    RepairFailed {
        path: PathBuf,
        #[source]
        source: Box<CoreError>,
    },

    /// The damaged database was moved to `quarantined`, where it is kept, but a later step failed
    /// (installing the repaired copy, creating or upgrading the new database). The quarantine is
    /// recorded next to the moved file, so the next open finishes the repair and still reports it.
    #[error(
        "{path} was damaged, so Ghost moved it to {quarantined}, where it is kept, but could not finish setting up its replacement: {reason}. Ghost will finish the repair the next time it opens this profile."
    )]
    RecoveryIncomplete {
        path: PathBuf,
        quarantined: PathBuf,
        /// What failed, phrased for this situation (a migration error's own message would claim the
        /// database "was left unchanged", which is not true of the one that was moved).
        reason: String,
        #[source]
        source: Box<CoreError>,
    },

    /// Another Ghost process (a second window, the CLI) has the database open or is opening it,
    /// and this open needed something that would disturb it.
    #[error("{path} is in use by another Ghost process (another window or the CLI), {reason}")]
    DatabaseInUse { path: PathBuf, reason: InUseReason },

    /// A folder Ghost keeps data in is a link (to a folder on another disk, say) that doesn't
    /// lead to a folder. It is left as it is: a new folder in its place would start the data over
    /// on this disk and leave the history behind the link for when the disk is back.
    #[error("{link} is a link to {target}, {problem}")]
    BrokenLink {
        link: PathBuf,
        target: PathBuf,
        problem: LinkProblem,
    },

    /// Another Ghost process (a second launch with the same data folder, or the CLI) owns this
    /// profile's sessions ([`crate::owner::ProfileOwner`]). `path` is the profile's folder.
    #[error(
        "Ghost is already running with the profile in {}. Quit it first, or start this copy with its own GHOST_DATA_DIR.",
        path.display()
    )]
    ProfileInUse { path: PathBuf },

    #[error("\"{0}\" is not a valid profile id")]
    InvalidProfileId(String),

    #[error("There is no profile with id \"{0}\"")]
    ProfileNotFound(String),

    #[error(
        "Could not find a folder for app data on this system. Set GHOST_DATA_DIR to choose one."
    )]
    NoDataDir,
}

/// Why [`CoreError::DatabaseInUse`] stopped an open. Each reads as the end of that error's sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InUseReason {
    /// The other process was still opening the database (perhaps repairing or upgrading it, which
    /// can take a while for a large one) when this open stopped waiting.
    Opening,
    /// This open found damage, but repairing means moving the file, which can't happen while
    /// another process uses it. The damage is recorded, so the next open that has the database to
    /// itself repairs it.
    Repair,
    /// This version needs to upgrade the database, which would change it underneath the other
    /// (older) process.
    Upgrade,
}

impl std::fmt::Display for InUseReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            InUseReason::Opening => {
                "which is still opening it (it may be repairing or upgrading it). Try again in a moment."
            }
            InUseReason::Repair => {
                "so the damage found in it could not be repaired yet. Close the other Ghost windows and the CLI, then open Ghost again to repair it."
            }
            InUseReason::Upgrade => {
                "so it could not be upgraded for this version of Ghost. Close the other Ghost windows and the CLI, then try again."
            }
        })
    }
}

/// What is wrong with where a [`CoreError::BrokenLink`] leads. Each reads as the end of that error's
/// sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkProblem {
    /// Nothing is there, most often because the disk it is on isn't connected.
    Missing,
    /// Something other than a folder is there.
    NotAFolder,
}

impl std::fmt::Display for LinkProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            LinkProblem::Missing => {
                "which can't be found, perhaps because it is on a disk that isn't connected. Reconnect the disk and try again, or remove the link to have Ghost start over with an empty folder there."
            }
            LinkProblem::NotAFolder => {
                "which is not a folder. Point the link at a folder, or remove it to have Ghost start over with an empty folder there."
            }
        })
    }
}

impl CoreError {
    /// Shorthand for mapping an `io::Error` at a known path.
    pub(crate) fn io(path: impl Into<PathBuf>) -> impl FnOnce(std::io::Error) -> CoreError {
        let path = path.into();
        move |source| CoreError::Io { path, source }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_readable_sentences() {
        let io = CoreError::Io {
            path: PathBuf::from("/data/state.json"),
            source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        };
        assert!(
            io.to_string()
                .starts_with("Could not access /data/state.json: ")
        );

        let too_new = CoreError::SchemaTooNew {
            found: 3,
            supported: 1,
        };
        assert_eq!(
            too_new.to_string(),
            "This data was saved by a newer version of Ghost (database version 3; this version reads up to 1). Update Ghost to open it."
        );

        assert_eq!(
            CoreError::InvalidProfileId("../x".into()).to_string(),
            "\"../x\" is not a valid profile id"
        );
        assert!(CoreError::NoDataDir.to_string().contains("GHOST_DATA_DIR"));
    }

    #[test]
    fn io_errors_keep_their_source() {
        use std::error::Error as _;
        let err = CoreError::io("/x")(std::io::Error::from(std::io::ErrorKind::NotFound));
        assert!(err.source().is_some());
    }

    #[test]
    fn repair_messages_say_where_the_data_is() {
        use std::error::Error as _;
        let incomplete = CoreError::RecoveryIncomplete {
            path: PathBuf::from("/p/aura.db"),
            quarantined: PathBuf::from("/p/backups/aura.db.corrupt-5"),
            reason: "upgrading it failed: no such table: x".into(),
            source: Box::new(CoreError::Migration("no such table: x".into())),
        };
        let text = incomplete.to_string();
        assert!(text.contains("/p/backups/aura.db.corrupt-5"), "{text}");
        assert!(!text.contains("left unchanged"), "{text}");
        assert!(incomplete.source().is_some());

        let failed = CoreError::RepairFailed {
            path: PathBuf::from("/p/aura.db"),
            source: Box::new(CoreError::Io {
                path: PathBuf::from("/p/aura.db.salvage.partial"),
                source: std::io::Error::other("disk full"),
            }),
        };
        assert!(
            failed
                .to_string()
                .starts_with("Ghost found damage in /p/aura.db but could not repair it, so the file was left where it is: ")
        );

        for problem in [LinkProblem::Missing, LinkProblem::NotAFolder] {
            let text = CoreError::BrokenLink {
                link: PathBuf::from("/data/profiles"),
                target: PathBuf::from("/Volumes/Backup/profiles"),
                problem,
            }
            .to_string();
            assert!(
                text.starts_with("/data/profiles is a link to /Volumes/Backup/profiles, which "),
                "{text}"
            );
            assert!(text.contains("remove"), "says how to start over: {text}");
            assert!(text.ends_with('.'), "{text}");
        }

        for reason in [
            InUseReason::Opening,
            InUseReason::Repair,
            InUseReason::Upgrade,
        ] {
            let text = CoreError::DatabaseInUse {
                path: PathBuf::from("/p/aura.db"),
                reason,
            }
            .to_string();
            assert!(
                text.starts_with("/p/aura.db is in use by another Ghost process"),
                "{text}"
            );
            assert!(text.ends_with('.'), "{text}");
        }
    }
}
