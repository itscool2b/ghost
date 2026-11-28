//! Fetching the SDK packages the pane needs into Ghost's own SDK folder.
//!
//! Every byte comes from `dl.google.com` over HTTPS with no redirect followed. An archive is
//! written to a scratch folder, checked against the size and checksum Google's index states, and
//! only then unpacked; its entries may not leave the folder they are unpacked into. A package
//! appears in the SDK folder whole or not at all.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::Error;
use crate::repo::{Checksum, Choice, HOST, Host, IMAGE_INDEX, ImagePath, Index, MAIN_INDEX};

/// The largest index document read (they are under 1 MiB).
const INDEX_LIMIT: usize = 8 * 1024 * 1024;
/// The most an archive may unpack to. The largest system image is about 8 GiB.
const UNPACK_LIMIT: u64 = 24 * 1024 * 1024 * 1024;
/// The scratch folder inside the SDK folder.
const SCRATCH: &str = ".fetch";

/// Which packages are missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Need {
    pub adb: bool,
    pub emulator: bool,
    pub image: bool,
}

impl Need {
    pub fn nothing(&self) -> bool {
        !(self.adb || self.emulator || self.image)
    }
}

/// One package to fetch and where it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub choice: Choice,
    /// Its folder under the SDK's root, with `/` between parts (`platform-tools`).
    pub folder: String,
}

/// What a fetch will do, for the person to read before it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub items: Vec<Item>,
    /// Google's SDK licence, which covers every item.
    pub license: String,
}

impl Plan {
    /// Bytes to download.
    pub fn download_size(&self) -> u64 {
        self.items.iter().map(|item| item.choice.archive.size).sum()
    }
}

/// Where a fetch is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    /// The package being worked on.
    pub name: String,
    pub phase: Phase,
    /// Bytes downloaded so far, across every package.
    pub received: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Downloading,
    Checking,
    Unpacking,
}

/// A client that speaks only HTTPS and follows no redirect.
pub fn client() -> Result<reqwest::Client, Error> {
    reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .user_agent("ghost")
        .build()
        .map_err(|error| Error::Download(error.to_string()))
}

/// Refuses any address that is not a file on Google's download site.
fn checked_url(url: &str) -> Result<reqwest::Url, Error> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|_| Error::Refused("That is not a download address.".into()))?;
    let allowed = parsed.scheme() == "https"
        && parsed.host_str() == Some(HOST)
        && parsed.port().is_none()
        && parsed.username().is_empty()
        && parsed.password().is_none();
    if allowed {
        Ok(parsed)
    } else {
        Err(Error::Refused(
            "Ghost downloads Android's tools only from dl.google.com.".into(),
        ))
    }
}

async fn get(client: &reqwest::Client, url: &str) -> Result<reqwest::Response, Error> {
    let response = client
        .get(checked_url(url)?)
        .send()
        .await
        .map_err(|error| Error::Download(plain(&error)))?;
    if response.status() != reqwest::StatusCode::OK {
        return Err(Error::Download(format!(
            "Google's download site answered {} for {}.",
            response.status().as_u16(),
            url.rsplit('/').next().unwrap_or(url)
        )));
    }
    Ok(response)
}

/// A network error without the address repeated in it.
fn plain(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "The connection timed out.".into()
    } else if error.is_connect() {
        "Google's download site could not be reached. Check the internet connection.".into()
    } else {
        "The connection was interrupted.".into()
    }
}

async fn index(client: &reqwest::Client, url: &str) -> Result<Index, Error> {
    let mut response = get(client, url).await?;
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| Error::Download(plain(&error)))?
    {
        if body.len() + chunk.len() > INDEX_LIMIT {
            return Err(Error::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Index::parse(&String::from_utf8_lossy(&body), url)
}

/// Reads Google's index and chooses the newest stable release of each missing package.
pub async fn plan(client: &reqwest::Client, host: Host, need: Need) -> Result<Plan, Error> {
    let missing = |what: &str| {
        Error::Download(format!(
            "Google does not publish {what} for this kind of computer."
        ))
    };
    let mut items = Vec::new();
    let mut license = None;
    if need.adb || need.emulator {
        let main = index(client, MAIN_INDEX).await?;
        if need.adb {
            let choice = main
                .newest("platform-tools", host)
                .ok_or_else(|| missing("adb"))?;
            items.push(Item {
                choice,
                folder: "platform-tools".into(),
            });
        }
        if need.emulator {
            let choice = main
                .newest("emulator", host)
                .ok_or_else(|| missing("the Android emulator"))?;
            items.push(Item {
                choice,
                folder: "emulator".into(),
            });
        }
        license = main.license;
    }
    if need.image {
        let images = index(client, IMAGE_INDEX).await?;
        let choice = images
            .newest_image(host)
            .ok_or_else(|| missing("an Android system image"))?;
        let folder = ImagePath::parse(&choice.path)
            .ok_or_else(|| Error::Protocol("Google's index named a package oddly.".into()))?
            .folder();
        items.push(Item { choice, folder });
        license = license.or(images.license);
    }
    let license = license
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| Error::Protocol("Google's index did not include its licence.".into()))?;
    Ok(Plan { items, license })
}

/// Carries out `plan` into the SDK at `root`. `report` hears about progress; setting `cancel`
/// stops it at the next chunk and leaves nothing half-installed.
pub async fn fetch(
    client: &reqwest::Client,
    plan: &Plan,
    root: &Path,
    report: &(dyn Fn(Progress) + Send + Sync),
    cancel: &AtomicBool,
) -> Result<(), Error> {
    let scratch = root.join(SCRATCH);
    // Whatever an earlier, interrupted fetch left is not trusted.
    let _ = tokio::fs::remove_dir_all(&scratch).await;
    tokio::fs::create_dir_all(&scratch).await?;
    let total = plan.download_size();
    let mut before = 0u64;
    let result = async {
        for (number, item) in plan.items.iter().enumerate() {
            let archive = scratch.join(format!("{number}.zip"));
            download(client, item, &archive, before, total, report, cancel).await?;
            before += item.choice.archive.size;
            report(Progress {
                name: item.choice.name.clone(),
                phase: Phase::Unpacking,
                received: before,
                total,
            });
            let unpacked = scratch.join(format!("{number}"));
            let (from, into) = (archive.clone(), unpacked.clone());
            tokio::task::spawn_blocking(move || unpack(&from, &into))
                .await
                .map_err(|_| Error::Protocol("Unpacking stopped unexpectedly.".into()))??;
            if cancel.load(Ordering::Acquire) {
                return Err(Error::Cancelled);
            }
            place(&unpacked, &root.join(Path::new(&item.folder))).await?;
            let _ = tokio::fs::remove_file(&archive).await;
        }
        Ok(())
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&scratch).await;
    result
}

/// Downloads one archive and checks it.
async fn download(
    client: &reqwest::Client,
    item: &Item,
    to: &Path,
    before: u64,
    total: u64,
    report: &(dyn Fn(Progress) + Send + Sync),
    cancel: &AtomicBool,
) -> Result<(), Error> {
    use tokio::io::AsyncWriteExt;
    let expected = &item.choice.archive;
    let name = &item.choice.name;
    let mut response = get(client, &expected.url).await?;
    let mut file = tokio::fs::File::create(to).await?;
    let mut hasher = Hasher::new(&expected.checksum);
    let mut received = 0u64;
    let mut reported = 0u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| Error::Download(plain(&error)))?
    {
        if cancel.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        received += chunk.len() as u64;
        if received > expected.size {
            return Err(Error::Download(format!(
                "{name} is larger than Google's index says."
            )));
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
        if received - reported >= 1024 * 1024 || received == expected.size {
            reported = received;
            report(Progress {
                name: name.clone(),
                phase: Phase::Downloading,
                received: before + received,
                total,
            });
        }
    }
    file.flush().await?;
    drop(file);
    report(Progress {
        name: name.clone(),
        phase: Phase::Checking,
        received: before + received,
        total,
    });
    if received != expected.size {
        return Err(Error::Download(format!(
            "The download of {name} was cut short."
        )));
    }
    if !hasher.matches(&expected.checksum) {
        return Err(Error::Download(format!(
            "{name} did not match the checksum in Google's index, so it was thrown away."
        )));
    }
    Ok(())
}

enum Hasher {
    Sha1(sha1::Sha1),
    Sha256(sha2::Sha256),
}

impl Hasher {
    fn new(checksum: &Checksum) -> Hasher {
        match checksum {
            Checksum::Sha1(_) => Hasher::Sha1(<sha1::Sha1 as sha1::Digest>::new()),
            Checksum::Sha256(_) => Hasher::Sha256(<sha2::Sha256 as sha2::Digest>::new()),
        }
    }

    fn update(&mut self, data: &[u8]) {
        match self {
            Hasher::Sha1(hasher) => sha1::Digest::update(hasher, data),
            Hasher::Sha256(hasher) => sha2::Digest::update(hasher, data),
        }
    }

    fn matches(self, checksum: &Checksum) -> bool {
        let hex = |bytes: &[u8]| {
            bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        match (self, checksum) {
            (Hasher::Sha1(hasher), Checksum::Sha1(expected)) => {
                hex(&sha1::Digest::finalize(hasher)) == *expected
            }
            (Hasher::Sha256(hasher), Checksum::Sha256(expected)) => {
                hex(&sha2::Digest::finalize(hasher)) == *expected
            }
            _ => false,
        }
    }
}

/// Unpacks a checked archive into the empty folder `into` (see `ghost_fetch::unzip`: no entry may
/// land outside it, by its own path or through a link), in the installer's own errors.
fn unpack(archive: &Path, into: &Path) -> Result<(), Error> {
    ghost_fetch::unzip(archive, into, UNPACK_LIMIT).map_err(|error| match error {
        ghost_fetch::UnzipError::TooLarge => Error::TooLarge,
        ghost_fetch::UnzipError::Io(error) => Error::Io(error),
        ghost_fetch::UnzipError::Archive(why) => Error::Download(why.into()),
    })
}

/// Moves the one folder an archive unpacked to into its place in the SDK, replacing what was
/// there.
async fn place(unpacked: &Path, destination: &Path) -> Result<(), Error> {
    let mut entries = tokio::fs::read_dir(unpacked).await?;
    let mut top: Vec<PathBuf> = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        top.push(entry.path());
    }
    let [folder] = top.as_slice() else {
        return Err(Error::Download(
            "The downloaded archive is not laid out as expected.".into(),
        ));
    };
    if !folder.is_dir() {
        return Err(Error::Download(
            "The downloaded archive is not laid out as expected.".into(),
        ));
    }
    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    if tokio::fs::try_exists(destination).await.unwrap_or(false) {
        tokio::fs::remove_dir_all(destination).await?;
    }
    tokio::fs::rename(folder, destination).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use zip::write::SimpleFileOptions;

    use super::*;

    fn archive(path: &Path, entries: &[(&str, &[u8])]) {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, data) in entries {
            if name.ends_with('/') {
                zip.add_directory(name.trim_end_matches('/'), SimpleFileOptions::default())
                    .unwrap();
            } else {
                zip.start_file(*name, SimpleFileOptions::default().unix_permissions(0o755))
                    .unwrap();
                zip.write_all(data).unwrap();
            }
        }
        zip.finish().unwrap();
    }

    #[test]
    fn only_files_on_googles_site_may_be_fetched() {
        assert!(checked_url("https://dl.google.com/android/repository/x.zip").is_ok());
        for bad in [
            "http://dl.google.com/android/repository/x.zip",
            "https://dl.google.com.evil.example/x.zip",
            "https://evil.example/dl.google.com/x.zip",
            "https://user:pass@dl.google.com/x.zip",
            "https://dl.google.com:8443/x.zip",
            "file:///etc/passwd",
            "not a url",
        ] {
            assert!(checked_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_archive_unpacks_into_its_folder() {
        let folder = tempfile::tempdir().unwrap();
        let zip = folder.path().join("a.zip");
        archive(
            &zip,
            &[
                ("platform-tools/", b""),
                ("platform-tools/adb", b"binary"),
                ("platform-tools/lib/x.so", b"lib"),
            ],
        );
        let into = folder.path().join("out");
        unpack(&zip, &into).unwrap();
        assert_eq!(
            std::fs::read(into.join("platform-tools/adb")).unwrap(),
            b"binary"
        );
        assert_eq!(
            std::fs::read(into.join("platform-tools/lib/x.so")).unwrap(),
            b"lib"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(into.join("platform-tools/adb"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o755);
        }
    }

    #[test]
    fn an_entry_that_leaves_the_folder_stops_the_unpack() {
        let folder = tempfile::tempdir().unwrap();
        for name in ["../escaped.txt", "/abs.txt", "a/../../escaped.txt"] {
            let zip = folder.path().join("bad.zip");
            archive(&zip, &[(name, b"x")]);
            let into = folder.path().join("out");
            let _ = std::fs::remove_dir_all(&into);
            // Either the entry is refused, or its name is made safe and it stays inside.
            let _ = unpack(&zip, &into);
            assert!(!folder.path().join("escaped.txt").exists(), "{name}");
            assert!(!Path::new("/abs.txt").exists(), "{name}");
        }
        assert!(unpack(&folder.path().join("missing.zip"), &folder.path().join("o")).is_err());
        std::fs::write(folder.path().join("junk.zip"), b"not a zip").unwrap();
        assert!(unpack(&folder.path().join("junk.zip"), &folder.path().join("o")).is_err());
    }

    #[tokio::test]
    async fn a_package_replaces_the_one_before_it_whole() {
        let folder = tempfile::tempdir().unwrap();
        let unpacked = folder.path().join("u");
        std::fs::create_dir_all(unpacked.join("emulator/lib")).unwrap();
        std::fs::write(unpacked.join("emulator/emulator"), b"new").unwrap();
        let destination = folder.path().join("sdk/emulator");
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::write(destination.join("stale"), b"old").unwrap();
        place(&unpacked, &destination).await.unwrap();
        assert_eq!(std::fs::read(destination.join("emulator")).unwrap(), b"new");
        assert!(!destination.join("stale").exists());

        // Two folders at the top, or a file, is not a package.
        let odd = folder.path().join("odd");
        std::fs::create_dir_all(odd.join("a")).unwrap();
        std::fs::create_dir_all(odd.join("b")).unwrap();
        assert!(place(&odd, &folder.path().join("sdk/x")).await.is_err());
        let file = folder.path().join("file");
        std::fs::create_dir_all(&file).unwrap();
        std::fs::write(file.join("only"), b"x").unwrap();
        assert!(place(&file, &folder.path().join("sdk/y")).await.is_err());
    }

    #[test]
    fn checksums_are_compared_in_full() {
        let sha1_of_abc = "a9993e364706816aba3e25717850c26c9cd0d89d";
        let sha256_of_abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        for (checksum, wrong) in [
            (
                Checksum::Sha1(sha1_of_abc.into()),
                Checksum::Sha1("0".repeat(40)),
            ),
            (
                Checksum::Sha256(sha256_of_abc.into()),
                Checksum::Sha256("0".repeat(64)),
            ),
        ] {
            let mut hasher = Hasher::new(&checksum);
            hasher.update(b"a");
            hasher.update(b"bc");
            assert!(hasher.matches(&checksum));
            let mut hasher = Hasher::new(&checksum);
            hasher.update(b"abd");
            assert!(!hasher.matches(&checksum));
            let mut hasher = Hasher::new(&wrong);
            hasher.update(b"abc");
            assert!(!hasher.matches(&wrong));
        }
        let mut mixed = Hasher::new(&Checksum::Sha1(sha1_of_abc.into()));
        mixed.update(b"abc");
        assert!(!mixed.matches(&Checksum::Sha256(sha256_of_abc.into())));
    }

    #[test]
    fn a_plan_adds_up_its_downloads() {
        let item = |size: u64| {
            Item {
            choice: Choice {
                path: "p".into(),
                name: "P".into(),
                revision: (1, 0, 0),
                archive: crate::repo::Index::parse(
                    &format!(
                        "<r><remotePackage path=\"p\"><revision><major>1</major></revision><uses-license ref=\"android-sdk-license\"/><channelRef ref=\"channel-0\"/><archives><archive><complete><size>{size}</size><checksum type=\"sha1\">{}</checksum><url>p.zip</url></complete></archive></archives></remotePackage></r>",
                        "a".repeat(40)
                    ),
                    MAIN_INDEX,
                )
                .unwrap()
                .newest("p", Host::current().unwrap())
                .unwrap()
                .archive,
            },
            folder: "p".into(),
        }
        };
        let plan = Plan {
            items: vec![item(10), item(32)],
            license: "terms".into(),
        };
        assert_eq!(plan.download_size(), 42);
        assert!(Need::default().nothing());
        assert!(
            !Need {
                image: true,
                ..Need::default()
            }
            .nothing()
        );
    }
}
