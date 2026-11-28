//! One pinned file over HTTPS.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sha2::Digest as _;

/// The digest a publisher lists for a file, in lowercase hex.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Digest<'a> {
    Sha256(&'a str),
    Sha512(&'a str),
}

/// Exactly which file: where it is, how long it is, and what it hashes to. All three are
/// constants of the build that asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pin<'a> {
    pub url: &'a str,
    pub size: u64,
    pub digest: Digest<'a>,
}

/// How far a download is, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub received: u64,
    pub total: u64,
}

/// Why a download did not arrive whole and correct. The caller words it for the person: it knows
/// what was being fetched and from whom.
#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("the address is not one this download may use")]
    Refused,
    #[error("the connection timed out")]
    Timeout,
    #[error("the server could not be reached")]
    Connect,
    #[error("the download was interrupted")]
    Interrupted,
    #[error("the server answered {0}")]
    Status(u16),
    #[error("the download is larger than the pinned file")]
    TooLarge,
    #[error("the download was cut short")]
    CutShort,
    #[error("the download did not match the pinned checksum")]
    Mismatch,
    #[error("stopped")]
    Cancelled,
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// Whether `url` is HTTPS on the default port, with no credentials, to one of `hosts`.
fn allowed(url: &reqwest::Url, hosts: &[&str]) -> bool {
    url.scheme() == "https"
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.host_str().is_some_and(|host| hosts.contains(&host))
}

/// An HTTPS client that follows at most five redirects, each to one of `hosts` only.
pub fn client(hosts: &'static [&'static str]) -> Result<reqwest::Client, DownloadError> {
    reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() < 5 && allowed(attempt.url(), hosts) {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .user_agent("ghost")
        .build()
        .map_err(|_| DownloadError::Connect)
}

enum Hasher {
    Sha256(sha2::Sha256),
    Sha512(sha2::Sha512),
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The bytes of a pinned file as they arrive: counted against the pinned size and hashed, so the
/// whole can be judged without reading it again. It does the judging [`download`] relies on, apart
/// from the network.
pub struct Checked<'a> {
    pin: Pin<'a>,
    hasher: Hasher,
    received: u64,
}

impl<'a> Checked<'a> {
    pub fn new(pin: Pin<'a>) -> Self {
        Self {
            hasher: match pin.digest {
                Digest::Sha256(_) => Hasher::Sha256(sha2::Sha256::new()),
                Digest::Sha512(_) => Hasher::Sha512(sha2::Sha512::new()),
            },
            pin,
            received: 0,
        }
    }

    pub fn received(&self) -> u64 {
        self.received
    }

    /// Takes the next bytes. More than the pinned size is refused before it is counted in, so
    /// the caller never writes it.
    pub fn take(&mut self, chunk: &[u8]) -> Result<(), DownloadError> {
        let next = self.received.saturating_add(chunk.len() as u64);
        if next > self.pin.size {
            return Err(DownloadError::TooLarge);
        }
        match &mut self.hasher {
            Hasher::Sha256(hasher) => hasher.update(chunk),
            Hasher::Sha512(hasher) => hasher.update(chunk),
        }
        self.received = next;
        Ok(())
    }

    /// Whether everything taken is the pinned file: its size, then its digest.
    pub fn finish(self) -> Result<(), DownloadError> {
        if self.received != self.pin.size {
            return Err(DownloadError::CutShort);
        }
        let (found, pinned) = match (self.hasher, self.pin.digest) {
            (Hasher::Sha256(hasher), Digest::Sha256(pinned)) => (hex(&hasher.finalize()), pinned),
            (Hasher::Sha512(hasher), Digest::Sha512(pinned)) => (hex(&hasher.finalize()), pinned),
            _ => return Err(DownloadError::Mismatch),
        };
        if found == pinned {
            Ok(())
        } else {
            Err(DownloadError::Mismatch)
        }
    }
}

/// Downloads the pinned file to `to` and checks its size and digest. `to` holds the pinned file
/// only when this returns `Ok`: on any failure it is removed. `report` hears how far the download
/// is, at most every half megabyte; `cancel` stops it at the next chunk.
pub async fn download(
    client: &reqwest::Client,
    hosts: &[&str],
    pin: Pin<'_>,
    to: &Path,
    report: &(dyn Fn(Progress) + Send + Sync),
    cancel: &AtomicBool,
) -> Result<(), DownloadError> {
    let result = fetch(client, hosts, pin, to, report, cancel).await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(to).await;
    }
    result
}

async fn fetch(
    client: &reqwest::Client,
    hosts: &[&str],
    pin: Pin<'_>,
    to: &Path,
    report: &(dyn Fn(Progress) + Send + Sync),
    cancel: &AtomicBool,
) -> Result<(), DownloadError> {
    use tokio::io::AsyncWriteExt;
    let url: reqwest::Url = pin.url.parse().map_err(|_| DownloadError::Refused)?;
    if !allowed(&url, hosts) {
        return Err(DownloadError::Refused);
    }
    let plain = |error: reqwest::Error| {
        if error.is_timeout() {
            DownloadError::Timeout
        } else if error.is_connect() {
            DownloadError::Connect
        } else {
            DownloadError::Interrupted
        }
    };
    let mut response = client.get(url).send().await.map_err(plain)?;
    if response.status() != reqwest::StatusCode::OK {
        return Err(DownloadError::Status(response.status().as_u16()));
    }
    let mut file = tokio::fs::File::create(to).await?;
    let mut checked = Checked::new(pin);
    let mut reported = 0u64;
    report(Progress {
        received: 0,
        total: pin.size,
    });
    while let Some(chunk) = response.chunk().await.map_err(plain)? {
        if cancel.load(Ordering::Acquire) {
            return Err(DownloadError::Cancelled);
        }
        checked.take(&chunk)?;
        file.write_all(&chunk).await?;
        let received = checked.received();
        if received - reported >= 512 * 1024 || received == pin.size {
            reported = received;
            report(Progress {
                received,
                total: pin.size,
            });
        }
    }
    file.flush().await?;
    drop(file);
    checked.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(bytes: &[u8]) -> (String, String) {
        (
            hex(&sha2::Sha256::digest(bytes)),
            hex(&sha2::Sha512::digest(bytes)),
        )
    }

    #[test]
    fn only_the_pinned_bytes_pass_under_either_digest() {
        let content = b"a pinned file, in two chunks";
        let (sha256, sha512) = digest(content);
        for pinned in [Digest::Sha256(&sha256), Digest::Sha512(&sha512)] {
            let pin = Pin {
                url: "https://example.org/file",
                size: content.len() as u64,
                digest: pinned,
            };
            let mut whole = Checked::new(pin);
            whole.take(&content[..9]).unwrap();
            whole.take(&content[9..]).unwrap();
            assert_eq!(whole.received(), pin.size);
            assert!(whole.finish().is_ok());

            let mut short = Checked::new(pin);
            short.take(&content[..9]).unwrap();
            assert!(matches!(short.finish(), Err(DownloadError::CutShort)));

            // The right length with one byte changed.
            let mut changed = content.to_vec();
            changed[3] ^= 1;
            let mut wrong = Checked::new(pin);
            wrong.take(&changed).unwrap();
            assert!(matches!(wrong.finish(), Err(DownloadError::Mismatch)));
        }
    }

    #[test]
    fn more_than_the_pinned_size_is_refused_before_it_is_counted() {
        let content = b"exactly this";
        let (sha256, _) = digest(content);
        let mut checked = Checked::new(Pin {
            url: "https://example.org/file",
            size: content.len() as u64,
            digest: Digest::Sha256(&sha256),
        });
        checked.take(content).unwrap();
        assert!(matches!(checked.take(b"!"), Err(DownloadError::TooLarge)));
        assert_eq!(
            checked.received(),
            content.len() as u64,
            "nothing more was taken"
        );
        assert!(checked.finish().is_ok(), "what was taken is still the file");
    }

    #[test]
    fn only_https_to_a_named_host_on_the_default_port_is_allowed() {
        let hosts = ["github.com", "objects.githubusercontent.com"];
        let ok = |text: &str| allowed(&text.parse().unwrap(), &hosts);
        assert!(ok(
            "https://github.com/godotengine/godot/releases/download/x/y.zip"
        ));
        assert!(ok("https://objects.githubusercontent.com/a?b=c"));
        for refused in [
            "http://github.com/a",
            "https://github.com:8443/a",
            "https://user:secret@github.com/a",
            "https://user@github.com/a",
            "https://evil.github.com/a",
            "https://github.com.evil.example/a",
            "https://example.org/a",
            "ftp://github.com/a",
        ] {
            assert!(!ok(refused), "{refused}");
        }
    }

    #[tokio::test]
    async fn an_address_outside_the_named_hosts_is_refused_and_nothing_is_written() {
        let folder = tempfile::tempdir().unwrap();
        let to = folder.path().join("file");
        let client = client(&["github.com"]).unwrap();
        let cancel = AtomicBool::new(false);
        for url in [
            "https://example.org/file",
            "http://github.com/file",
            "not a url",
        ] {
            let refused = download(
                &client,
                &["github.com"],
                Pin {
                    url,
                    size: 1,
                    digest: Digest::Sha256("00"),
                },
                &to,
                &|_| panic!("nothing is fetched"),
                &cancel,
            )
            .await;
            assert!(matches!(refused, Err(DownloadError::Refused)), "{url}");
            assert!(!to.exists());
        }
    }
}
