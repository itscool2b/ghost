//! The larger dictation model (docs/local-dictation.md, "The larger model"): Parakeet Ultra,
//! fetched once after installation, because it is too large to ride in the installer. o7 fetches
//! it by itself a minute after it starts (owner, 22 November 2026: "Automatically pls"), until
//! the person says no in Settings > Voice.
//!
//! What may happen here, and how it is held to that:
//! - The app itself starts the download, or its own page does; only that page cancels or removes
//!   it. Neither supplies anything: the address, the size and the SHA-256 are constants of this
//!   build ([`model::LARGER`]).
//! - Cancel and Remove are remembered as a file beside the model ([`decline`]): o7 never starts
//!   the download by itself again until the person presses Download. It also leaves it alone
//!   where the disk would be left short of room ([`HEADROOM`]).
//! - The connection is HTTPS only and follows a redirect only to Hugging Face's own hosts.
//! - The file is written beside its final name as `.partial`, never past the pinned size, and gets
//!   its name only after all of it hashed to the pinned SHA-256. Dictation loads nothing else.
//! - It is weights for the speech engine: nothing in it is run.
use super::model::{self, Download};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What the page hears while the larger model arrives. It ends with `Ready`, `Cancelled` or
/// `Failed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LargerEvent {
    Downloading {
        received: u32,
        total: u32,
    },
    /// All of it arrived; its SHA-256 is being compared with the pinned one.
    Verifying,
    Ready,
    /// Stopped by the person. What arrived is kept, and the next download continues from it.
    Cancelled,
    Failed {
        message: String,
    },
}

const INTERRUPTED: &str =
    "The download was interrupted. Download again to continue from where it stopped.";
const OFFLINE: &str = "The download could not start. Check your connection, then try again.";
const UNAVAILABLE: &str = "Hugging Face did not send the model. Try again later.";
const NOT_PINNED: &str =
    "The download is not the file o7 expects, so it was discarded. Nothing was installed.";
const DISK: &str =
    "The model could not be saved. Check that this disk has 500 MB free, then try again.";

/// A download in flight: the wish to stop it, and how much of the file is here so far, for a
/// page that did not start it.
#[derive(Default)]
struct Run {
    cancel: AtomicBool,
    received: AtomicU64,
}

/// The download in flight, if any: one at a time, whoever asked.
static ACTIVE: Mutex<Option<Arc<Run>>> = Mutex::new(None);

/// How long after launch o7 first looks at fetching the model by itself, and how long it waits
/// before it looks again (it was offline, the disk was short, a download was already running).
pub const FIRST: Duration = Duration::from_secs(60);
pub const AGAIN: Duration = Duration::from_secs(15 * 60);
/// What the disk must still have free once the model is on it, for o7's own data and everything
/// else on the computer, before o7 fetches the model unasked.
const HEADROOM: u64 = 2 << 30;
/// Its presence in the models folder says the person does not want the model fetched for them.
const DECLINED: &str = "larger-model.declined";

/// Whether this copy fetches the model by itself at all. A development build does not, since
/// each scratch data folder would fetch its own 486 MB; `O7_DICTATION_AUTOMATIC=1` turns it on
/// there to test it, and `=0` turns it off anywhere.
pub fn enabled() -> bool {
    enabled_for(
        std::env::var("O7_DICTATION_AUTOMATIC").ok().as_deref(),
        cfg!(debug_assertions),
    )
}

fn enabled_for(wish: Option<&str>, development: bool) -> bool {
    match wish {
        Some("0") => false,
        Some("1") => true,
        _ => !development,
    }
}

/// The person said no: they cancelled the download or removed the model.
pub fn declined(folder: &Path) -> bool {
    folder.join(DECLINED).symlink_metadata().is_ok()
}

/// Remembers that the person does not want the model fetched for them.
pub fn decline(folder: &Path) -> Result<(), String> {
    std::fs::create_dir_all(folder)
        .and_then(|()| File::create(folder.join(DECLINED)))
        .map(|_| ())
        .map_err(|_| "o7 could not save that choice. Check that this disk is not full.".to_string())
}

/// The person asked for the model: o7 may fetch it by itself again, should this attempt not
/// finish.
pub fn accept(folder: &Path) -> Result<(), String> {
    let path = folder.join(DECLINED);
    match std::fs::remove_file(&path).or_else(|_| std::fs::remove_dir(&path)) {
        Ok(()) => Ok(()),
        Err(_) if !declined(folder) => Ok(()),
        Err(_) => Err("o7 could not save that choice.".into()),
    }
}

/// Whether fetching the rest of the model leaves the disk [`HEADROOM`]. A disk that will not say
/// how much it has free is tried: the writing fails cleanly where there is no room.
fn room(free: Option<u64>, kept: u64, pin: &Download) -> bool {
    free.is_none_or(|free| free >= pin.bytes.saturating_sub(kept).saturating_add(HEADROOM))
}

/// Bytes free to this user on the disk holding `folder`, which may not exist yet.
fn free_space(folder: &Path) -> Option<u64> {
    let existing = folder.ancestors().find(|path| path.exists())?;
    disk::free(existing)
}

/// What `folder` and its disk say about fetching the model unasked: it is not here, nobody said
/// no, and `free` is room enough.
fn open_to(folder: &Path, free: Option<u64>) -> bool {
    let pin = &model::LARGER;
    model::larger(Some(folder)).is_none()
        && !declined(folder)
        && room(
            free,
            std::fs::metadata(Arriving::partial(folder, pin)).map_or(0, |meta| meta.len()),
            pin,
        )
}

/// Whether o7 will fetch the model by itself. What Settings says and what the launch does both
/// ask here.
pub fn wanted(folder: &Path) -> bool {
    enabled() && super::audio::cpu_supported() && open_to(folder, free_space(folder))
}

/// What the launch does after looking: nothing more this run, or look again after [`AGAIN`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attempt {
    Done,
    Later,
}

/// How an automatic download's ending decides the next step. A connection that failed is tried
/// again; a file that was not the pinned one, or a disk that would not take it, is not fetched
/// again and again behind the person's back.
fn after(end: &LargerEvent) -> Attempt {
    match end {
        LargerEvent::Failed { message }
            if [INTERRUPTED, OFFLINE, UNAVAILABLE].contains(&message.as_str()) =>
        {
            Attempt::Later
        }
        // Cancelled is the person's no, which `decline` wrote down. It is looked at again only so
        // that a later yes in the same run is honoured.
        LargerEvent::Cancelled => Attempt::Later,
        _ => Attempt::Done,
    }
}

/// One look at fetching the model unasked, and the fetch itself when everything allows it.
pub async fn attempt(folder: PathBuf) -> Attempt {
    if !enabled() || !super::audio::cpu_supported() || model::larger(Some(&folder)).is_some() {
        return Attempt::Done;
    }
    if downloading() {
        return Attempt::Later;
    }
    let free = free_space(&folder);
    if !open_to(&folder, free) {
        tracing::debug!(
            declined = declined(&folder),
            ?free,
            "the larger dictation model is not fetched by itself for now"
        );
        return Attempt::Later;
    }
    let (ended, end) = tokio::sync::oneshot::channel();
    let ended = Mutex::new(Some(ended));
    let started = start(folder, move |event| {
        if matches!(
            event,
            LargerEvent::Ready | LargerEvent::Cancelled | LargerEvent::Failed { .. }
        ) && let Some(ended) = crate::sync::lock(&ended).take()
        {
            let _ = ended.send(event);
        }
    });
    if started.is_err() {
        return Attempt::Later;
    }
    match end.await {
        Ok(event) => {
            if let LargerEvent::Failed { message } = &event {
                tracing::debug!(%message, "the larger dictation model did not arrive by itself");
            }
            after(&event)
        }
        Err(_) => Attempt::Later,
    }
}

/// Whether a redirect or the first request may go to `url`: HTTPS to Hugging Face or one of its
/// file hosts, on the default port, with no credentials in the address.
pub(crate) fn model_host(url: &reqwest::Url) -> bool {
    let host = url.host_str().unwrap_or_default();
    url.scheme() == "https"
        && (host == "huggingface.co"
            || host.ends_with(".huggingface.co")
            || host.ends_with(".hf.co"))
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
}

/// The file as it arrives: `<name>.partial` beside where it will live, with its hash and length
/// kept as it grows.
pub(crate) struct Arriving {
    pin: &'static Download,
    folder: PathBuf,
    file: File,
    hash: Sha256,
    received: u64,
}

impl Arriving {
    fn partial(folder: &Path, pin: &Download) -> PathBuf {
        folder.join(format!("{}.partial", pin.file))
    }

    /// Opens the partial file, keeping what an earlier attempt left when it can still be the
    /// start of the pinned file: its bytes are hashed again, so nothing is taken on trust.
    pub fn open(folder: &Path, pin: &'static Download) -> std::io::Result<Self> {
        std::fs::create_dir_all(folder)?;
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(Self::partial(folder, pin))?;
        let mut hash = Sha256::new();
        let mut received = file.metadata()?.len();
        if received >= pin.bytes {
            // As long as the whole file or longer, yet never given its name: not the pinned file.
            received = 0;
            file.set_len(0)?;
        } else if received > 0 {
            let mut buffer = vec![0_u8; 1 << 20];
            file.seek(SeekFrom::Start(0))?;
            loop {
                let read = file.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                hash.update(&buffer[..read]);
            }
        }
        file.seek(SeekFrom::Start(received))?;
        Ok(Self {
            pin,
            folder: folder.to_path_buf(),
            file,
            hash,
            received,
        })
    }

    pub fn received(&self) -> u64 {
        self.received
    }

    /// The server sent the whole file instead of the rest: start over.
    pub fn restart(&mut self) -> std::io::Result<()> {
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        self.hash = Sha256::new();
        self.received = 0;
        Ok(())
    }

    /// Appends `chunk`. More than the pinned size is refused before a byte of it is written.
    pub fn write(&mut self, chunk: &[u8]) -> Result<(), &'static str> {
        if self.received + chunk.len() as u64 > self.pin.bytes {
            return Err(NOT_PINNED);
        }
        self.file.write_all(chunk).map_err(|_| DISK)?;
        self.hash.update(chunk);
        self.received += chunk.len() as u64;
        Ok(())
    }

    /// Gives the file its name, if it is the pinned one: the pinned length and the pinned SHA-256.
    /// Anything else is deleted.
    pub fn finish(self) -> Result<PathBuf, &'static str> {
        let Self {
            pin,
            folder,
            file,
            hash,
            received,
        } = self;
        let partial = Self::partial(&folder, pin);
        let digest: String = hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if received != pin.bytes || digest != pin.sha256 {
            drop(file);
            let _ = std::fs::remove_file(&partial);
            return Err(NOT_PINNED);
        }
        file.sync_all().map_err(|_| DISK)?;
        drop(file);
        let target = folder.join(pin.file);
        std::fs::rename(&partial, &target).map_err(|_| DISK)?;
        Ok(target)
    }
}

/// Starts the download unless one is running. `emit` gets every event; a page that left stops
/// hearing them and the download goes on, since the file is as useful to the next page.
pub fn start(
    folder: PathBuf,
    emit: impl Fn(LargerEvent) + Send + Sync + 'static,
) -> Result<(), String> {
    let run = Arc::new(Run::default());
    {
        let mut active = crate::sync::lock(&ACTIVE);
        if active.is_some() {
            return Err("The larger model is already downloading.".into());
        }
        *active = Some(run.clone());
    }
    tauri::async_runtime::spawn(async move {
        let event = match fetch(&folder, &model::LARGER, &run, &emit).await {
            Ok(true) => LargerEvent::Ready,
            Ok(false) => LargerEvent::Cancelled,
            Err(message) => LargerEvent::Failed {
                message: message.into(),
            },
        };
        *crate::sync::lock(&ACTIVE) = None;
        emit(event);
    });
    Ok(())
}

pub fn cancel() {
    if let Some(run) = crate::sync::lock(&ACTIVE).as_ref() {
        run.cancel.store(true, Ordering::Release);
    }
}

pub fn downloading() -> bool {
    crate::sync::lock(&ACTIVE).is_some()
}

/// How much of the model the download in flight has so far.
pub fn received() -> Option<u64> {
    crate::sync::lock(&ACTIVE)
        .as_ref()
        .map(|run| run.received.load(Ordering::Acquire))
}

/// Deletes the larger model and what a download left. Dictation returns to the built-in model.
pub fn remove(folder: &Path) -> Result<(), String> {
    if downloading() {
        return Err("Cancel the download first.".into());
    }
    // The engine maps the file; it is let go first, which fails only while a recording runs.
    super::engine::Engine::forget();
    let pin = &model::LARGER;
    for path in [folder.join(pin.file), Arriving::partial(folder, pin)] {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                return Err("The model is in use. Finish dictating, then remove it.".into());
            }
        }
    }
    Ok(())
}

/// `Ok(true)`: the model is in place. `Ok(false)`: cancelled, with what arrived kept.
async fn fetch(
    folder: &Path,
    pin: &'static Download,
    run: &Run,
    emit: &(impl Fn(LargerEvent) + Send + Sync),
) -> Result<bool, &'static str> {
    let url: reqwest::Url = pin.url.parse().map_err(|_| NOT_PINNED)?;
    if !model_host(&url) {
        return Err(NOT_PINNED);
    }
    let mut arriving = Arriving::open(folder, pin).map_err(|_| DISK)?;
    run.received.store(arriving.received(), Ordering::Release);
    let client = reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 {
                attempt.error("too many redirects")
            } else if model_host(attempt.url()) {
                attempt.follow()
            } else {
                attempt.error("redirected outside the model's hosts")
            }
        }))
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .user_agent(concat!("o7/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| "The download could not be set up.")?;
    let mut request = client.get(url);
    if arriving.received() > 0 {
        request = request.header(
            reqwest::header::RANGE,
            format!("bytes={}-", arriving.received()),
        );
    }
    let mut response = request.send().await.map_err(|_| OFFLINE)?;
    match response.status() {
        reqwest::StatusCode::PARTIAL_CONTENT => {}
        // The whole file, whatever was asked for: start over with it.
        reqwest::StatusCode::OK => arriving.restart().map_err(|_| DISK)?,
        _ => return Err(UNAVAILABLE),
    }
    if response
        .content_length()
        .is_some_and(|rest| arriving.received() + rest != pin.bytes)
    {
        return Err(NOT_PINNED);
    }
    let total = u32::try_from(pin.bytes).unwrap_or(u32::MAX);
    run.received.store(arriving.received(), Ordering::Release);
    let progress = |arriving: &Arriving| {
        emit(LargerEvent::Downloading {
            received: u32::try_from(arriving.received()).unwrap_or(u32::MAX),
            total,
        });
    };
    progress(&arriving);
    let mut told = Instant::now();
    loop {
        if run.cancel.load(Ordering::Acquire) {
            return Ok(false);
        }
        let Some(chunk) = response.chunk().await.map_err(|_| INTERRUPTED)? else {
            break;
        };
        arriving.write(&chunk)?;
        run.received.store(arriving.received(), Ordering::Release);
        if told.elapsed() >= Duration::from_millis(200) {
            told = Instant::now();
            progress(&arriving);
        }
    }
    if arriving.received() != pin.bytes {
        // Short of the whole file: the connection ended early. What arrived is kept.
        return Err(INTERRUPTED);
    }
    emit(LargerEvent::Verifying);
    arriving.finish()?;
    Ok(true)
}

/// The one question asked of the operating system here: how much room a disk has.
mod disk {
    use std::path::Path;

    #[cfg(windows)]
    #[allow(unsafe_code)] // Read-only volume query; the contract is stated at the call.
    pub fn free(existing: &Path) -> Option<u64> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        let wide: Vec<u16> = existing.as_os_str().encode_wide().chain([0]).collect();
        let mut free = 0_u64;
        // SAFETY: `wide` is a NUL-terminated path that outlives the call, `free` is a valid
        // place for the one number asked for, and the two null pointers are numbers not asked for.
        let answered = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        (answered != 0).then_some(free)
    }

    #[cfg(unix)]
    #[allow(unsafe_code)] // Read-only volume query; the contract is stated at the call.
    pub fn free(existing: &Path) -> Option<u64> {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(existing.as_os_str().as_bytes()).ok()?;
        let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: `path` is a NUL-terminated string that outlives the call and `stat` is room
        // for the one structure the call fills in.
        if unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
            return None;
        }
        // SAFETY: the call succeeded, so it filled the structure in.
        let stat = unsafe { stat.assume_init() };
        // The fields' widths differ between Linux and macOS.
        #[allow(clippy::unnecessary_cast)]
        Some((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
    }

    #[cfg(not(any(windows, unix)))]
    pub fn free(_existing: &Path) -> Option<u64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn o7_fetches_the_model_by_itself_only_until_the_person_says_no() {
        let root = tempfile::tempdir().unwrap();
        // The folder need not exist to be asked about, or to have the no written in it.
        let folder = root.path().join("models").join("dictation");
        let plenty = Some(u64::MAX);
        assert!(!declined(&folder));
        assert!(open_to(&folder, plenty), "nothing is in the way at first");
        assert_eq!(accept(&folder), Ok(()), "a yes with no no before it");
        decline(&folder).unwrap();
        assert!(declined(&folder));
        assert!(
            !open_to(&folder, plenty),
            "a no holds whatever else is true"
        );
        decline(&folder).unwrap();
        assert!(declined(&folder), "said twice is still no");
        accept(&folder).unwrap();
        assert!(open_to(&folder, plenty), "Download takes the no back");
        // Whatever sits under that name is a no, and a yes clears it.
        std::fs::create_dir(folder.join(DECLINED)).unwrap();
        assert!(!open_to(&folder, plenty));
        accept(&folder).unwrap();
        assert!(open_to(&folder, plenty));
        // The model already here is not fetched again.
        let model = File::create(folder.join(model::LARGER.file)).unwrap();
        model.set_len(model::LARGER.bytes).unwrap();
        assert!(!open_to(&folder, plenty));
    }

    #[test]
    fn a_development_build_fetches_nothing_unasked_and_any_build_can_be_told_not_to() {
        assert!(enabled_for(None, false), "a release does");
        assert!(!enabled_for(None, true), "a development build does not");
        assert!(enabled_for(Some("1"), true), "unless it is testing this");
        assert!(
            !enabled_for(Some("0"), false),
            "and a release can be told not to"
        );
        assert!(!enabled_for(Some("yes"), true), "nothing else turns it on");
    }

    #[test]
    fn the_model_is_not_fetched_unasked_onto_a_disk_it_would_leave_short() {
        let pin = &model::LARGER;
        assert!(room(Some(pin.bytes + HEADROOM), 0, pin));
        assert!(!room(Some(pin.bytes + HEADROOM - 1), 0, pin));
        assert!(!room(Some(0), 0, pin));
        // What already arrived needs no room again.
        assert!(room(Some(HEADROOM + 1000), pin.bytes - 1000, pin));
        assert!(!room(Some(HEADROOM + 999), pin.bytes - 1000, pin));
        assert!(room(None, 0, pin), "a disk that will not say is tried");
        let folder = tempfile::tempdir().unwrap();
        assert!(!open_to(folder.path(), Some(pin.bytes)));
        std::fs::write(Arriving::partial(folder.path(), pin), vec![0_u8; 4096]).unwrap();
        assert!(open_to(folder.path(), Some(pin.bytes - 4096 + HEADROOM)));
        assert!(!open_to(folder.path(), Some(pin.bytes - 4097 + HEADROOM)));
        // This disk says: the question reaches the operating system and comes back.
        assert!(free_space(&folder.path().join("not").join("made").join("yet")).is_some());
    }

    #[test]
    fn a_failed_connection_is_tried_again_and_a_wrong_file_is_not() {
        let failed = |message: &str| LargerEvent::Failed {
            message: message.into(),
        };
        for retried in [INTERRUPTED, OFFLINE, UNAVAILABLE] {
            assert_eq!(after(&failed(retried)), Attempt::Later, "{retried}");
        }
        for ended in [NOT_PINNED, DISK, "The download could not be set up."] {
            assert_eq!(after(&failed(ended)), Attempt::Done, "{ended}");
        }
        assert_eq!(after(&LargerEvent::Ready), Attempt::Done);
        assert_eq!(after(&LargerEvent::Cancelled), Attempt::Later);
    }

    fn digest(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// A pin for `content`, leaked so it lives as a build's constant does.
    fn pin(content: &[u8]) -> &'static Download {
        Box::leak(Box::new(Download {
            file: "model.gguf",
            name: "Test model",
            bytes: content.len() as u64,
            sha256: Box::leak(digest(content).into_boxed_str()),
            url: "https://huggingface.co/owner/repo/resolve/0123/model.gguf",
        }))
    }

    #[test]
    fn the_pinned_file_gets_its_name_only_once_all_of_it_is_there_and_matches() {
        let folder = tempfile::tempdir().unwrap();
        let content = b"the whole of the model, byte for byte".repeat(100);
        let pin = pin(&content);
        let mut arriving = Arriving::open(folder.path(), pin).unwrap();
        for chunk in content.chunks(97) {
            assert!(
                !folder.path().join(pin.file).exists(),
                "no name while it arrives"
            );
            arriving.write(chunk).unwrap();
        }
        let path = arriving.finish().unwrap();
        assert_eq!(std::fs::read(path).unwrap(), content);
        assert!(!Arriving::partial(folder.path(), pin).exists());
    }

    #[test]
    fn a_file_that_is_not_the_pinned_one_is_discarded() {
        let content = b"what was pinned".repeat(50);
        // Other bytes of the right length; one byte changed; cut short.
        let mut changed = content.clone();
        changed[100] ^= 1;
        for wrong in [
            vec![7_u8; content.len()],
            changed,
            content[..content.len() - 1].to_vec(),
        ] {
            let folder = tempfile::tempdir().unwrap();
            let pin = pin(&content);
            let mut arriving = Arriving::open(folder.path(), pin).unwrap();
            arriving.write(&wrong).unwrap();
            assert_eq!(arriving.finish(), Err(NOT_PINNED));
            assert!(
                !folder.path().join(pin.file).exists(),
                "it never gets the name"
            );
            assert!(
                !Arriving::partial(folder.path(), pin).exists(),
                "and is deleted"
            );
        }
    }

    #[test]
    fn more_than_the_pinned_size_is_refused_before_it_is_written() {
        let folder = tempfile::tempdir().unwrap();
        let content = b"exactly this much".to_vec();
        let pin = pin(&content);
        let mut arriving = Arriving::open(folder.path(), pin).unwrap();
        arriving.write(&content).unwrap();
        assert_eq!(arriving.write(b"x"), Err(NOT_PINNED));
        assert_eq!(arriving.received(), content.len() as u64);
        assert_eq!(
            std::fs::metadata(Arriving::partial(folder.path(), pin))
                .unwrap()
                .len(),
            content.len() as u64
        );
    }

    #[test]
    fn a_stopped_download_continues_from_what_it_kept_and_still_has_to_match() {
        let folder = tempfile::tempdir().unwrap();
        let content: Vec<u8> = (0..5000_u32).flat_map(u32::to_le_bytes).collect();
        let pin = pin(&content);
        let mut first = Arriving::open(folder.path(), pin).unwrap();
        first.write(&content[..7000]).unwrap();
        drop(first);
        let mut second = Arriving::open(folder.path(), pin).unwrap();
        assert_eq!(second.received(), 7000, "what arrived is kept");
        second.write(&content[7000..]).unwrap();
        assert_eq!(std::fs::read(second.finish().unwrap()).unwrap(), content);
        // What was kept is hashed again, not trusted: a changed byte in it fails the whole file.
        let mut third = Arriving::open(folder.path(), pin).unwrap();
        third.write(&content[..7000]).unwrap();
        drop(third);
        let partial = Arriving::partial(folder.path(), pin);
        let mut kept = std::fs::read(&partial).unwrap();
        kept[10] ^= 1;
        std::fs::write(&partial, kept).unwrap();
        std::fs::remove_file(folder.path().join(pin.file)).unwrap();
        let mut fourth = Arriving::open(folder.path(), pin).unwrap();
        fourth.write(&content[7000..]).unwrap();
        assert_eq!(fourth.finish(), Err(NOT_PINNED));
        assert!(!folder.path().join(pin.file).exists());
    }

    #[test]
    fn a_server_that_sends_the_whole_file_again_starts_it_over() {
        let folder = tempfile::tempdir().unwrap();
        let content = b"sent twice from the start".repeat(40);
        let pin = pin(&content);
        let mut first = Arriving::open(folder.path(), pin).unwrap();
        first.write(&content[..300]).unwrap();
        drop(first);
        let mut again = Arriving::open(folder.path(), pin).unwrap();
        again.restart().unwrap();
        assert_eq!(again.received(), 0);
        again.write(&content).unwrap();
        assert_eq!(std::fs::read(again.finish().unwrap()).unwrap(), content);
        // A leftover as long as the whole file never got its name: it is not kept.
        std::fs::write(
            Arriving::partial(folder.path(), pin),
            vec![1_u8; content.len() + 5],
        )
        .unwrap();
        assert_eq!(Arriving::open(folder.path(), pin).unwrap().received(), 0);
    }

    #[test]
    fn only_hugging_face_over_https_may_send_the_model() {
        let allowed = |url: &str| model_host(&url.parse().unwrap());
        assert!(allowed(model::LARGER.url), "the pinned address itself");
        assert!(allowed(
            "https://cas-bridge.xethub.hf.co/xet-bridge-us/abc?sig=1"
        ));
        assert!(allowed("https://cdn-lfs-us-1.hf.co/repos/ab/cd/file"));
        for refused in [
            "http://huggingface.co/owner/repo/resolve/main/model.gguf",
            "https://huggingface.co.example.com/model.gguf",
            "https://example.com/huggingface.co/model.gguf",
            "https://nothf.co/model.gguf",
            "https://hf.co.evil.example/model.gguf",
            "https://huggingface.co:8443/model.gguf",
            "https://127.0.0.1/model.gguf",
            "file:///c:/model.gguf",
        ] {
            assert!(!allowed(refused), "{refused}");
        }
        // Credentials in the address, put together here so no scanner reads one in the source.
        let signed_in = format!("https://{}:{}@huggingface.co/model.gguf", "someone", "word");
        assert!(!allowed(&signed_in));
    }

    #[test]
    fn removing_leaves_nothing_and_missing_files_are_no_error() {
        let folder = tempfile::tempdir().unwrap();
        let pin = &model::LARGER;
        assert_eq!(remove(folder.path()), Ok(()));
        std::fs::write(folder.path().join(pin.file), b"model").unwrap();
        std::fs::write(Arriving::partial(folder.path(), pin), b"part").unwrap();
        assert_eq!(remove(folder.path()), Ok(()));
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);
    }
}
