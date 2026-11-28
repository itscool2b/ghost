//! Fetching a pinned file: the two steps every tool Ghost downloads after installation goes
//! through, in one place.
//!
//! - [`download`] takes one file over HTTPS from hosts the caller names, never writes past the
//!   pinned size, and gives the file its name only when its size and digest are the pinned ones.
//! - [`unzip`] unpacks a checked archive into an empty folder so that no entry lands outside it:
//!   not by its own path, and not through a link.
//!
//! Neither decides what may be fetched or who may ask. That is the caller's: the pin is a
//! constant of the build, and the person starts the fetch.

mod download;
mod unzip;

pub use download::{Checked, Digest, DownloadError, Pin, Progress, client, download};
pub use unzip::{UnzipError, unzip};
