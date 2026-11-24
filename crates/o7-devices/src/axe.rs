//! AXe (<https://github.com/cameroncooke/AXe>), the tool o7 drives iPhone simulators with: their
//! outline, taps, typing, keys and the pictures the pane shows ([`crate::ios`]).
//!
//! It is fetched on first use from its own GitHub release, checked against the size and SHA-256
//! pinned here before anything in it is unpacked, and kept in o7's folder. Version 2.0.4 is
//! pinned on purpose: it is the last release under the MIT licence (later ones are GPL-3.0-only),
//! and it supports Xcode 26 and 27 (third-party/axe.md). o7 does not ship it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::Error;

pub const VERSION: &str = "2.0.4";
pub const URL: &str = "https://github.com/cameroncooke/AXe/releases/download/v2.0.4/AXe-macOS-v2.0.4-universal.tar.gz";
pub const SHA256: &str = "3178ec66d9ff46455fec737debad719c41cab16edb1435b2c721e07760a208c3";
/// The archive's size in bytes, so the download is known before it starts and capped.
pub const SIZE: u64 = 16_526_761;
/// The hosts a GitHub release download passes through. No other is followed.
const HOSTS: [&str; 3] = [
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

/// Where this version of AXe lives in o7's devices folder.
pub fn folder(root: &Path) -> PathBuf {
    root.join("ios").join("axe").join(VERSION)
}

/// AXe's program, when it has been fetched.
pub fn installed(root: &Path) -> Option<PathBuf> {
    let program = folder(root).join("axe");
    program.is_file().then_some(program)
}

/// How far a fetch is, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub received: u64,
    pub total: u64,
}

/// An HTTPS client that follows a release download's redirects to GitHub's own hosts only.
fn client() -> Result<reqwest::Client, Error> {
    reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let url = attempt.url();
            let allowed = attempt.previous().len() < 5
                && url.scheme() == "https"
                && url.port().is_none()
                && url.host_str().is_some_and(|host| HOSTS.contains(&host));
            if allowed {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .user_agent("o7")
        .build()
        .map_err(|error| Error::Download(error.to_string()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Fetches AXe into o7's folder unless it is there, and answers its program. `report` hears how
/// far the download is; `cancel` stops it at the next chunk and leaves nothing behind.
pub async fn fetch(
    root: &Path,
    report: &(dyn Fn(Progress) + Send + Sync),
    cancel: &AtomicBool,
) -> Result<PathBuf, Error> {
    if let Some(program) = installed(root) {
        return Ok(program);
    }
    if !cfg!(target_os = "macos") {
        return Err(Error::Refused(crate::ios::Missing::NotMac.to_string()));
    }
    let base = root.join("ios").join("axe");
    tokio::fs::create_dir_all(&base).await?;
    let archive = base.join(format!("download-{}.tar.gz", std::process::id()));
    let staging = base.join(format!("unpacking-{}", std::process::id()));
    let result = async {
        download(&archive, report, cancel).await?;
        let _ = tokio::fs::remove_dir_all(&staging).await;
        tokio::fs::create_dir_all(&staging).await?;
        // macOS's own tar, on an archive already checked against its pinned hash: it holds
        // exactly what was published, and tar refuses names with `..` or a leading slash.
        let mut command = tokio::process::Command::new("/usr/bin/tar");
        command.arg("-xzf").arg(&archive).arg("-C").arg(&staging);
        let ran = crate::ios::run(command, None, 64 * 1024, Duration::from_secs(180)).await?;
        if !ran.ok {
            return Err(Error::Download(
                "AXe's archive could not be unpacked.".into(),
            ));
        }
        if !staging.join("axe").is_file() {
            return Err(Error::Download("AXe's archive did not hold AXe.".into()));
        }
        let destination = folder(root);
        let _ = tokio::fs::remove_dir_all(&destination).await;
        tokio::fs::rename(&staging, &destination).await?;
        Ok(destination.join("axe"))
    }
    .await;
    let _ = tokio::fs::remove_file(&archive).await;
    let _ = tokio::fs::remove_dir_all(&staging).await;
    result
}

/// Downloads the pinned archive to `to` and checks its size and hash.
async fn download(
    to: &Path,
    report: &(dyn Fn(Progress) + Send + Sync),
    cancel: &AtomicBool,
) -> Result<(), Error> {
    use sha2::Digest;
    use tokio::io::AsyncWriteExt;
    let plain = |error: reqwest::Error| {
        Error::Download(if error.is_timeout() {
            "The connection to GitHub timed out.".into()
        } else if error.is_connect() {
            "GitHub could not be reached. Check the internet connection.".into()
        } else {
            "The download from GitHub was interrupted.".into()
        })
    };
    let mut response = client()?.get(URL).send().await.map_err(plain)?;
    if response.status() != reqwest::StatusCode::OK {
        return Err(Error::Download(format!(
            "GitHub answered {} for AXe {VERSION}.",
            response.status().as_u16()
        )));
    }
    let mut file = tokio::fs::File::create(to).await?;
    let mut hasher = sha2::Sha256::new();
    let mut received = 0u64;
    let mut reported = 0u64;
    report(Progress {
        received,
        total: SIZE,
    });
    while let Some(chunk) = response.chunk().await.map_err(plain)? {
        if cancel.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        received += chunk.len() as u64;
        if received > SIZE {
            return Err(Error::Download(
                "AXe's download is larger than the release o7 knows.".into(),
            ));
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
        if received - reported >= 512 * 1024 || received == SIZE {
            reported = received;
            report(Progress {
                received,
                total: SIZE,
            });
        }
    }
    file.flush().await?;
    drop(file);
    if received != SIZE {
        return Err(Error::Download("AXe's download was cut short.".into()));
    }
    if hex(&hasher.finalize()) != SHA256 {
        return Err(Error::Download(
            "AXe's download did not match the checksum o7 pins, so it was thrown away.".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pinned_release_is_one_file_of_one_version() {
        assert!(URL.starts_with("https://github.com/cameroncooke/AXe/releases/download/v"));
        assert!(
            URL.contains(&format!("/v{VERSION}/"))
                && URL.ends_with(&format!("v{VERSION}-universal.tar.gz"))
        );
        assert_eq!(SHA256.len(), 64);
        assert!(
            SHA256
                .chars()
                .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
        );
        const { assert!(SIZE > 1_000_000) };
        assert_eq!(hex(&[0x0a, 0xff]), "0aff");
    }

    #[tokio::test]
    async fn nothing_is_fetched_off_a_mac_or_over_what_is_there() {
        let folder = tempfile::tempdir().unwrap();
        assert_eq!(installed(folder.path()), None);
        let cancel = AtomicBool::new(false);
        if !cfg!(target_os = "macos") {
            let refused = fetch(folder.path(), &|_| {}, &cancel).await.unwrap_err();
            assert_eq!(refused.to_string(), "iPhone simulators run only on a Mac.");
            assert!(!folder.path().join("ios").exists(), "nothing was written");
        }
        // One already fetched is used as it is, with no download.
        let program = super::folder(folder.path()).join("axe");
        std::fs::create_dir_all(program.parent().unwrap()).unwrap();
        std::fs::write(&program, b"#!").unwrap();
        assert_eq!(
            fetch(folder.path(), &|_| panic!("no download"), &cancel)
                .await
                .unwrap(),
            program
        );
    }
}
