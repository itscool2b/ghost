//! Once a day, a copy of Ghost adds one to the site's count of copies that ran (`/api/count`,
//! `apps/web-worker/src/count.rs`).
//!
//! What is sent, and who decides:
//! - The request carries the version, the operating system, the processor type and whether this
//!   copy has never counted itself before. No identifier, nothing about the person, their
//!   projects or chats. The site stores only each US Eastern day's totals.
//! - It goes with the automatic update check and stops with it: turning that off in Settings turns
//!   this off. A development build, a staging build and a smoke launch never count.
//! - One small file in the data folder, `counted.json`, holds the moment before which the next
//!   count is not sent; the site says how long that is (the time to its next day).
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const COUNT: &str = "https://onghost.dev/api/count";
const FILE: &str = "counted.json";
/// A US Eastern day is 23 to 25 hours long; anything else in an answer is not believed.
const LONGEST_WAIT: u64 = 25 * 60 * 60;

#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
struct Counted {
    /// Seconds since 1970 before which no count is sent.
    next: u64,
}

/// Whether to count now, and whether it would be this copy's first count.
fn due(file: &Path, now: u64) -> Option<bool> {
    match std::fs::read(file) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(true),
        // A file that cannot be read or makes no sense: counted before, so not a first.
        Err(_) => Some(false),
        Ok(bytes) => match serde_json::from_slice::<Counted>(&bytes) {
            Ok(counted) => (now >= counted.next).then_some(false),
            Err(_) => Some(false),
        },
    }
}

fn query(version: &str, first: bool) -> String {
    format!(
        "os={}&arch={}&version={version}&first={}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        u8::from(first)
    )
}

/// The site answers with the seconds to its next day.
fn wait(answer: &str) -> Option<u64> {
    answer
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|seconds| (1..=LONGEST_WAIT).contains(seconds))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Counts this copy if it has not been counted today. Quiet: a failure only means trying again at
/// the next update check.
pub async fn count_once_a_day(data_dir: PathBuf, version: String) {
    if let Err(error) = count(COUNT, &data_dir.join(FILE), &version, now()).await {
        tracing::debug!(%error, "daily count not sent");
    }
}

async fn count(endpoint: &str, file: &Path, version: &str, now: u64) -> Result<(), String> {
    let Some(first) = due(file, now) else {
        return Ok(());
    };
    let client = reqwest::Client::builder()
        .https_only(endpoint.starts_with("https://"))
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .user_agent(concat!("ghost/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .get(format!("{endpoint}?{}", query(version, first)))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("the site answered {}", response.status()));
    }
    let answer = response.text().await.map_err(|error| error.to_string())?;
    let seconds = wait(&answer).ok_or("the site's answer is not a wait")?;
    let counted = Counted {
        next: now + seconds,
    };
    let bytes = serde_json::to_vec(&counted).map_err(|error| error.to_string())?;
    std::fs::write(file, bytes).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn a_copy_counts_once_until_the_site_says_the_day_is_over() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE);
        assert_eq!(due(&file, 100), Some(true), "never counted: a first");
        std::fs::write(&file, br#"{"next":200}"#).unwrap();
        assert_eq!(due(&file, 199), None);
        assert_eq!(due(&file, 200), Some(false));
        std::fs::write(&file, b"not json").unwrap();
        assert_eq!(due(&file, 0), Some(false), "a damaged file is not a first");
    }

    #[test]
    fn the_request_says_only_version_system_processor_and_first() {
        let query = query("0.1.1748", true);
        assert_eq!(
            query,
            format!(
                "os={}&arch={}&version=0.1.1748&first=1",
                std::env::consts::OS,
                std::env::consts::ARCH
            )
        );
        assert!(self::query("1.2.3", false).ends_with("&first=0"));
    }

    #[test]
    fn only_a_wait_of_up_to_a_long_day_is_believed() {
        assert_eq!(wait("7843\n"), Some(7843));
        assert_eq!(wait("90000"), Some(90_000));
        for answer in ["", "0", "-5", "90001", "soon", "<html>"] {
            assert_eq!(wait(answer), None, "{answer}");
        }
    }

    /// A loopback server that answers every request with `answer` and records the request lines.
    async fn site(answer: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}/api/count", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buffer = vec![0; 4096];
                let read = stream.read(&mut buffer).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                log.lock()
                    .unwrap()
                    .push(request.lines().next().unwrap_or_default().to_owned());
                let reply = format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                    answer.len()
                );
                let _ = stream.write_all(reply.as_bytes()).await;
            }
        });
        (address, seen)
    }

    #[tokio::test]
    async fn counts_first_then_waits_for_the_next_day_then_counts_again() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE);
        let (endpoint, seen) = site("3600\n").await;
        count(&endpoint, &file, "0.1.1748", 1000).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Counted>(&std::fs::read(&file).unwrap()).unwrap(),
            Counted { next: 4600 }
        );
        // Later the same day: nothing is sent.
        count(&endpoint, &file, "0.1.1748", 4599).await.unwrap();
        // The next day: counted again, not as a first.
        count(&endpoint, &file, "0.1.1748", 4600).await.unwrap();
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert!(seen[0].starts_with("GET /api/count?os="), "{}", seen[0]);
        assert!(
            seen[0].contains("&version=0.1.1748&first=1 "),
            "{}",
            seen[0]
        );
        assert!(seen[1].contains("&first=0 "), "{}", seen[1]);
    }

    #[tokio::test]
    async fn an_answer_that_is_not_a_wait_records_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE);
        let (endpoint, _) = site("<html>maintenance</html>").await;
        assert!(count(&endpoint, &file, "0.1.1748", 1000).await.is_err());
        assert!(
            !file.exists(),
            "tried again at the next check, still as a first"
        );
    }
}
