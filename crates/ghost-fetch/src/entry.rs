//! One file out of a published zip archive, fetched without the archive, and a folder packed
//! into a zip.
//!
//! Godot's export templates are one archive of 1.28 GB; the web template inside it is 10 MB.
//! A server that answers range requests gives that part alone. The part is pinned as a whole
//! file is: where it is, how long it is, what it hashes to. The pin is made once, from an
//! archive checked against the digest its publisher lists.

use std::io::{Read, Write};
use std::path::Path;

use crate::download::{Checked, Digest, DownloadError, Pin, allowed};

/// Exactly which part of which archive: where its entry begins, how long it is packed and
/// unpacked, and what it hashes to unpacked. All constants of the build that asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryPin<'a> {
    pub url: &'a str,
    /// Where the entry's own header begins in the archive.
    pub offset: u64,
    /// How many bytes the entry's data takes in the archive.
    pub packed: u64,
    /// How many bytes it is unpacked, and their SHA-256 in lowercase hex.
    pub size: u64,
    pub sha256: &'a str,
}

/// The most an entry's header may take before its data: the fixed part, a name and extras.
const HEADER_ROOM: u64 = 30 + 2048;
/// The largest part this will hold in memory.
const MOST: u64 = 256 * 1024 * 1024;

/// Unpacks one entry from the bytes of an archive that begin at its header. The entry must be
/// stored or deflated, and must unpack to exactly the pinned bytes.
pub fn unpack_entry(bytes: &[u8], pin: &EntryPin<'_>) -> Result<Vec<u8>, DownloadError> {
    let short = || DownloadError::CutShort;
    let fixed = bytes.get(..30).ok_or_else(short)?;
    if fixed[..4] != [0x50, 0x4b, 0x03, 0x04] {
        return Err(DownloadError::Mismatch);
    }
    let method = u16::from_le_bytes([fixed[8], fixed[9]]);
    let name = usize::from(u16::from_le_bytes([fixed[26], fixed[27]]));
    let extra = usize::from(u16::from_le_bytes([fixed[28], fixed[29]]));
    let start = 30 + name + extra;
    let packed = usize::try_from(pin.packed).map_err(|_| DownloadError::TooLarge)?;
    let data = bytes
        .get(start..start.checked_add(packed).ok_or(DownloadError::TooLarge)?)
        .ok_or_else(short)?;
    let size = usize::try_from(pin.size).map_err(|_| DownloadError::TooLarge)?;
    let mut plain = Vec::with_capacity(size.min(64 * 1024 * 1024));
    match method {
        0 => plain.extend_from_slice(data),
        8 => {
            // Never more than the pinned size, whatever the data says it unpacks to.
            flate2::read::DeflateDecoder::new(data)
                .take(pin.size + 1)
                .read_to_end(&mut plain)
                .map_err(|_| DownloadError::Mismatch)?;
        }
        _ => return Err(DownloadError::Mismatch),
    }
    let mut checked = Checked::new(Pin {
        url: pin.url,
        size: pin.size,
        digest: Digest::Sha256(pin.sha256),
    });
    checked.take(&plain)?;
    checked.finish()?;
    Ok(plain)
}

/// Fetches the pinned entry alone and writes it, unpacked and checked, to `to`. `to` holds the
/// pinned file only when this returns `Ok`.
pub async fn download_entry(
    client: &reqwest::Client,
    hosts: &[&str],
    pin: EntryPin<'_>,
    to: &Path,
) -> Result<(), DownloadError> {
    let url: reqwest::Url = pin.url.parse().map_err(|_| DownloadError::Refused)?;
    if !allowed(&url, hosts) {
        return Err(DownloadError::Refused);
    }
    if pin.packed > MOST || pin.size > MOST {
        return Err(DownloadError::TooLarge);
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
    let last = pin.offset + HEADER_ROOM + pin.packed - 1;
    let mut response = client
        .get(url)
        .header(
            reqwest::header::RANGE,
            format!("bytes={}-{last}", pin.offset),
        )
        .send()
        .await
        .map_err(plain)?;
    // The whole archive, or anything else, is not what was asked for.
    if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(DownloadError::Status(response.status().as_u16()));
    }
    let begins = response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("bytes "))
        .and_then(|value| value.split('-').next())
        .and_then(|start| start.parse::<u64>().ok());
    if begins != Some(pin.offset) {
        return Err(DownloadError::Mismatch);
    }
    let room = HEADER_ROOM + pin.packed;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(plain)? {
        if bytes.len() as u64 + chunk.len() as u64 > room {
            return Err(DownloadError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    // Inflating ten megabytes is work for a thread that may block: the pin goes there by value.
    let (url, sha256) = (pin.url.to_owned(), pin.sha256.to_owned());
    let (offset, packed, size) = (pin.offset, pin.packed, pin.size);
    let unpacked = tokio::task::spawn_blocking(move || {
        unpack_entry(
            &bytes,
            &EntryPin {
                url: &url,
                offset,
                packed,
                size,
                sha256: &sha256,
            },
        )
    })
    .await
    .map_err(|_| DownloadError::Interrupted)??;
    // Under another name until it is whole, so a file with the right name is always the file.
    let part = to.with_extension("part");
    tokio::fs::write(&part, &unpacked).await?;
    if let Err(error) = tokio::fs::rename(&part, to).await {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(error.into());
    }
    Ok(())
}

/// Why a folder was not packed.
#[derive(Debug, thiserror::Error)]
pub enum ZipError {
    #[error("The folder holds too much to pack.")]
    TooLarge,
    #[error("The folder holds nothing to pack.")]
    Empty,
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("The archive could not be written.")]
    Write,
}

/// Packs the files of `folder` into a zip at `to`, each under its path inside the folder, so
/// that what is at the folder's top is at the archive's top. Links, hidden entries and the
/// names in `skip` are left out. At most `limit` bytes are read. Answers how many files went
/// in. It blocks: call it from a thread that may.
pub fn zip_folder(folder: &Path, to: &Path, skip: &[&str], limit: u64) -> Result<u32, ZipError> {
    let mut found: Vec<(String, std::path::PathBuf)> = Vec::new();
    let mut waiting = vec![(folder.to_owned(), String::new())];
    while let Some((directory, prefix)) = waiting.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let kind = entry.file_type()?;
            if kind.is_symlink() || name.starts_with('.') || skip.contains(&name.as_str()) {
                continue;
            }
            if kind.is_dir() {
                waiting.push((entry.path(), format!("{prefix}{name}/")));
            } else if kind.is_file() && entry.path() != to {
                found.push((format!("{prefix}{name}"), entry.path()));
            }
            if found.len() > 20_000 {
                return Err(ZipError::TooLarge);
            }
        }
    }
    if found.is_empty() {
        return Err(ZipError::Empty);
    }
    found.sort();
    let part = to.with_extension("part");
    let result = (|| {
        let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(std::fs::File::create(&part)?));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o644);
        let mut read = 0u64;
        let mut buffer = vec![0u8; 256 * 1024];
        for (name, path) in &found {
            zip.start_file(name.as_str(), options)
                .map_err(|_| ZipError::Write)?;
            let mut file = std::fs::File::open(path)?;
            loop {
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                read += count as u64;
                if read > limit {
                    return Err(ZipError::TooLarge);
                }
                zip.write_all(&buffer[..count])?;
            }
        }
        zip.finish().map_err(|_| ZipError::Write)?.flush()?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            std::fs::rename(&part, to)?;
            Ok(u32::try_from(found.len()).unwrap_or(u32::MAX))
        }
        Err(error) => {
            let _ = std::fs::remove_file(&part);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest as _;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// An archive with one deflated entry, and where that entry begins and how long it is.
    fn archive(name: &str, content: &[u8]) -> (Vec<u8>, u64, u64) {
        let mut bytes = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut bytes);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            // Something before it, so the entry does not begin the archive.
            zip.start_file("first.txt", options).unwrap();
            zip.write_all(b"before").unwrap();
            zip.start_file(name, options).unwrap();
            zip.write_all(content).unwrap();
            zip.finish().unwrap();
        }
        let bytes = bytes.into_inner();
        let mut read = zip::ZipArchive::new(std::io::Cursor::new(bytes.clone())).unwrap();
        let entry = read.by_name(name).unwrap();
        let (offset, packed) = (entry.header_start(), entry.compressed_size());
        drop(entry);
        (bytes, offset, packed)
    }

    #[test]
    fn an_entry_is_unpacked_from_its_own_bytes_and_checked() {
        let content: Vec<u8> = (0..40_000u32)
            .flat_map(|n| (n % 251).to_le_bytes())
            .collect();
        let (bytes, offset, packed) = archive("templates/web.zip", &content);
        let sha256 = hex(&sha2::Sha256::digest(&content));
        let pin = EntryPin {
            url: "https://example.test/a.zip",
            offset,
            packed,
            size: content.len() as u64,
            sha256: &sha256,
        };
        let from = &bytes[offset as usize..];
        assert_eq!(unpack_entry(from, &pin).unwrap(), content);
        // With room to spare after it, as a range request returns.
        let mut roomy = from.to_vec();
        roomy.extend_from_slice(&[7; 500]);
        assert_eq!(unpack_entry(&roomy, &pin).unwrap(), content);

        // Another hash, another size, another place, a cut answer: none is taken.
        let wrong = "0".repeat(64);
        assert!(matches!(
            unpack_entry(
                from,
                &EntryPin {
                    sha256: &wrong,
                    ..pin
                }
            ),
            Err(DownloadError::Mismatch)
        ));
        assert!(
            unpack_entry(
                from,
                &EntryPin {
                    size: pin.size - 1,
                    ..pin
                }
            )
            .is_err()
        );
        assert!(
            unpack_entry(
                from,
                &EntryPin {
                    size: pin.size + 1,
                    ..pin
                }
            )
            .is_err()
        );
        assert!(matches!(
            unpack_entry(&bytes[offset as usize + 1..], &pin),
            Err(DownloadError::Mismatch)
        ));
        assert!(matches!(
            unpack_entry(&from[..from.len() / 2], &EntryPin { packed, ..pin }),
            Err(DownloadError::CutShort | DownloadError::Mismatch)
        ));
        assert!(matches!(
            unpack_entry(&[], &pin),
            Err(DownloadError::CutShort)
        ));
    }

    #[tokio::test]
    async fn an_entry_is_fetched_only_from_a_host_it_may_use() {
        let client = crate::client(&["example.test"]).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let to = dir.path().join("web.zip");
        let pin = EntryPin {
            url: "https://elsewhere.test/a.zip",
            offset: 0,
            packed: 10,
            size: 10,
            sha256: "00",
        };
        assert!(matches!(
            download_entry(&client, &["example.test"], pin, &to).await,
            Err(DownloadError::Refused)
        ));
        let plain = EntryPin {
            url: "http://example.test/a.zip",
            ..pin
        };
        assert!(matches!(
            download_entry(&client, &["example.test"], plain, &to).await,
            Err(DownloadError::Refused)
        ));
        assert!(!to.exists());
    }

    #[test]
    fn a_folder_is_packed_with_its_top_at_the_archives_top() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("out");
        std::fs::create_dir_all(folder.join("assets/img")).unwrap();
        std::fs::create_dir_all(folder.join("node_modules/x")).unwrap();
        std::fs::create_dir_all(folder.join(".git")).unwrap();
        std::fs::write(folder.join("index.html"), b"<html>").unwrap();
        std::fs::write(folder.join("index.wasm"), vec![3u8; 70_000]).unwrap();
        std::fs::write(folder.join("assets/img/a.png"), b"png").unwrap();
        std::fs::write(folder.join("node_modules/x/index.js"), b"no").unwrap();
        std::fs::write(folder.join(".git/config"), b"no").unwrap();
        std::fs::write(folder.join(".env"), b"no").unwrap();
        // Packed into the folder it packs: the archive is not put inside itself.
        let to = folder.join("game-web.zip");
        assert_eq!(
            zip_folder(&folder, &to, &["node_modules"], 1 << 20).unwrap(),
            3
        );
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&to).unwrap()).unwrap();
        let mut names: Vec<String> = zip.file_names().map(str::to_owned).collect();
        names.sort();
        assert_eq!(names, ["assets/img/a.png", "index.html", "index.wasm"]);
        let mut wasm = Vec::new();
        zip.by_name("index.wasm")
            .unwrap()
            .read_to_end(&mut wasm)
            .unwrap();
        assert_eq!(wasm, vec![3u8; 70_000]);
        // Again over the first: the new one replaces it.
        assert_eq!(
            zip_folder(&folder, &to, &["node_modules"], 1 << 20).unwrap(),
            3
        );

        // More than the limit, and nothing at all.
        assert!(matches!(
            zip_folder(&folder, &dir.path().join("small.zip"), &[], 1000),
            Err(ZipError::TooLarge)
        ));
        assert!(!dir.path().join("small.zip").exists());
        assert!(!dir.path().join("small.part").exists());
        let empty = dir.path().join("empty");
        std::fs::create_dir_all(empty.join(".hidden")).unwrap();
        assert!(matches!(
            zip_folder(&empty, &dir.path().join("e.zip"), &[], 1000),
            Err(ZipError::Empty)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_the_folder_is_not_packed() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), b"someone else's").unwrap();
        let folder = dir.path().join("out");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("index.html"), b"<html>").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret.txt"), folder.join("link.txt"))
            .unwrap();
        std::os::unix::fs::symlink(outside.path(), folder.join("linked")).unwrap();
        let to = dir.path().join("out.zip");
        assert_eq!(zip_folder(&folder, &to, &[], 1 << 20).unwrap(), 1);
    }
}
