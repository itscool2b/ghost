//! Photos a paired device stages for a message ([`crate::wire::Op::UploadBegin`]): in memory
//! only, and for its own session only. Each arrives in pieces of [`UPLOAD_CHUNK`] bytes, in
//! order, and is checked against the SHA-256 the device gave before anything is done with it. A
//! staged photo goes when a message carried it, when the device drops it, when nothing touched
//! it for [`EXPIRES`], or when its session ends. The room promised to it is given back only once
//! nothing holds its bytes any more, a message being sent with it included, so the computer's
//! room bounds what photos take in memory.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use base64::Engine as _;
use o7_protocol::session::PromptAck;
use sha2::{Digest as _, Sha256};

use super::lock;
use crate::{
    backend::{Backend, BackendError, PreparedImage},
    crypto,
    wire::{MAX_PHOTOS, UPLOAD_CHUNK},
};

/// The largest photo, in bytes.
pub(crate) const MAX_UPLOAD: usize = 8 * 1024 * 1024;
/// Photos arriving at once in one session: begun, and not yet whole.
pub(crate) const MAX_OPEN: usize = 4;
/// Photos staged in one session, arriving or whole.
pub(crate) const MAX_STAGED: usize = 16;
/// What one session's staged photos may hold, and every session's on this computer together.
pub(crate) const SESSION_BYTES: usize = 24 * 1024 * 1024;
pub(crate) const COMPUTER_BYTES: usize = 64 * 1024 * 1024;
/// A staged photo nothing touched for this long is dropped.
pub(crate) const EXPIRES: Duration = Duration::from_secs(10 * 60);
/// The types a photo may be.
const MIMES: [&str; 4] = ["image/png", "image/jpeg", "image/webp", "image/gif"];
/// The longest name kept, in characters.
const MAX_NAME: usize = 128;
/// The most base64 one piece takes: [`UPLOAD_CHUNK`] bytes, padded.
const MAX_PIECE: usize = UPLOAD_CHUNK.div_ceil(3) * 4;

/// The room promised to one photo, given back when its last holder lets go: the stage, or a
/// message still being sent with it.
#[derive(Debug)]
struct Promise {
    size: usize,
    session: Arc<AtomicUsize>,
    computer: Arc<AtomicUsize>,
}

impl Drop for Promise {
    fn drop(&mut self) {
        self.session.fetch_sub(self.size, Ordering::SeqCst);
        self.computer.fetch_sub(self.size, Ordering::SeqCst);
    }
}

/// A photo staged whole, as a message takes it.
#[derive(Debug, Clone)]
pub(crate) struct Staged {
    pub bytes: Arc<Vec<u8>>,
    pub name: String,
    pub mime: &'static str,
    /// Its room, held as long as this copy is.
    _promise: Arc<Promise>,
}

struct Upload {
    name: String,
    mime: &'static str,
    size: usize,
    sha256: [u8; 32],
    hasher: Sha256,
    received: Vec<u8>,
    /// The index the next piece must have.
    next: u32,
    /// The photo, once whole and checked.
    whole: Option<Arc<Vec<u8>>>,
    touched: Instant,
    /// A message is being sent with it.
    sending: bool,
    promise: Arc<Promise>,
}

/// One session's staged photos, and what they hold of this computer's room for them.
pub(crate) struct Uploads {
    staged: HashMap<String, Upload>,
    /// Bytes promised to this session's photos: each one's whole size, from the moment it began
    /// until nothing holds it.
    bytes: Arc<AtomicUsize>,
    /// Bytes promised to every session's photos on this computer.
    computer: Arc<AtomicUsize>,
}

fn invalid(message: impl Into<String>) -> BackendError {
    BackendError::new("invalid", message)
}

fn busy(message: &str) -> BackendError {
    BackendError::new("busy", message)
}

fn too_large(message: &str) -> BackendError {
    BackendError::new("too_large", message)
}

/// 64 hex digits, either case, as 32 bytes.
fn digest(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 32];
    for (at, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex.get(at * 2..at * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// Promises `size` more bytes of the computer's room, when that much is left.
fn promise(computer: &AtomicUsize, size: usize) -> bool {
    let mut held = computer.load(Ordering::SeqCst);
    loop {
        if held + size > COMPUTER_BYTES {
            return false;
        }
        match computer.compare_exchange(held, held + size, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return true,
            Err(now) => held = now,
        }
    }
}

impl Uploads {
    pub(crate) fn new(computer: Arc<AtomicUsize>) -> Self {
        Self {
            staged: HashMap::new(),
            bytes: Arc::new(AtomicUsize::new(0)),
            computer,
        }
    }

    /// Starts staging a photo and answers its id. Its whole size is promised to it at once, so
    /// a session or the computer is never asked to hold more than its room.
    pub(crate) fn begin(
        &mut self,
        name: &str,
        mime: &str,
        size: u64,
        sha256: &str,
        now: Instant,
    ) -> Result<String, BackendError> {
        let mime = MIMES
            .iter()
            .find(|known| **known == mime)
            .copied()
            .ok_or_else(|| invalid("A photo is a PNG, JPEG, WebP or GIF."))?;
        let sha256 = digest(sha256).ok_or_else(|| invalid("The SHA-256 is 64 hex digits."))?;
        let size = usize::try_from(size).unwrap_or(usize::MAX);
        if size == 0 {
            return Err(invalid("The photo is empty."));
        }
        if size > MAX_UPLOAD {
            return Err(too_large("A photo is at most 8 MiB."));
        }
        let open = self
            .staged
            .values()
            .filter(|upload| upload.whole.is_none())
            .count();
        if open >= MAX_OPEN || self.staged.len() >= MAX_STAGED {
            return Err(busy(
                "Too many photos are staged at once. Send or remove some first.",
            ));
        }
        if self.bytes.load(Ordering::SeqCst) + size > SESSION_BYTES {
            return Err(busy(
                "This device has too many photos waiting to be sent. Send or remove some first.",
            ));
        }
        // Checked and promised in one step, so sessions beginning at once cannot pass the
        // computer's room together.
        if !promise(&self.computer, size) {
            return Err(busy(
                "This computer has too many photos waiting. Try again in a moment.",
            ));
        }
        let Ok(random) = crypto::random::<9>() else {
            self.computer.fetch_sub(size, Ordering::SeqCst);
            return Err(BackendError::new(
                "unavailable",
                "The system's random source is unavailable.",
            ));
        };
        let id = crypto::encode(&random);
        let name = crate::registry::one_line(name, MAX_NAME);
        self.bytes.fetch_add(size, Ordering::SeqCst);
        let promise = Arc::new(Promise {
            size,
            session: self.bytes.clone(),
            computer: self.computer.clone(),
        });
        self.staged.insert(
            id.clone(),
            Upload {
                name: if name.is_empty() {
                    "Photo".into()
                } else {
                    name
                },
                mime,
                size,
                sha256,
                hasher: Sha256::new(),
                // As much as was promised, at once: what arrives never moves it.
                received: Vec::with_capacity(size),
                next: 0,
                whole: None,
                touched: now,
                sending: false,
                promise,
            },
        );
        Ok(id)
    }

    /// The next piece of photo `id`: how many bytes have arrived, and whether it is whole. A
    /// photo that arrived whole but not as it was sent is dropped (`corrupt`).
    pub(crate) fn chunk(
        &mut self,
        id: &str,
        index: u32,
        data: &str,
        now: Instant,
    ) -> Result<(usize, bool), BackendError> {
        let upload = self.staged.get_mut(id).ok_or_else(|| {
            BackendError::new(
                "not_found",
                "No photo is staged under that id. Begin it again.",
            )
        })?;
        if upload.whole.is_some() {
            return Err(invalid("That photo has arrived whole already."));
        }
        if index != upload.next {
            return Err(invalid(format!(
                "The next piece of that photo is number {}.",
                upload.next
            )));
        }
        if data.len() > MAX_PIECE {
            return Err(too_large("A piece is at most 48 KiB."));
        }
        let piece = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|_| invalid("A piece is standard base64."))?;
        if piece.is_empty() {
            return Err(invalid("A piece is empty."));
        }
        if piece.len() > UPLOAD_CHUNK {
            return Err(too_large("A piece is at most 48 KiB."));
        }
        if upload.received.len() + piece.len() > upload.size {
            return Err(too_large("That is more than the photo's size."));
        }
        upload.hasher.update(&piece);
        upload.received.extend_from_slice(&piece);
        upload.next += 1;
        upload.touched = now;
        let received = upload.received.len();
        if received < upload.size {
            return Ok((received, false));
        }
        let sum: [u8; 32] = upload.hasher.finalize_reset().into();
        if sum != upload.sha256 {
            self.remove(id);
            return Err(BackendError::new(
                "corrupt",
                "The photo did not arrive as it was sent. Send it again.",
            ));
        }
        upload.whole = Some(Arc::new(std::mem::take(&mut upload.received)));
        Ok((received, true))
    }

    /// Drops photo `id`, if it is staged.
    pub(crate) fn abort(&mut self, id: &str) {
        self.remove(id);
    }

    /// The whole photos `ids` names, held for one message until it is sent or fails.
    fn hold(&mut self, ids: &[String]) -> Result<Vec<Staged>, BackendError> {
        for (at, id) in ids.iter().enumerate() {
            if ids[..at].contains(id) {
                return Err(invalid("A photo is named twice."));
            }
            let upload = self.staged.get(id).ok_or_else(|| {
                BackendError::new(
                    "not_found",
                    "A photo is no longer staged on this computer. Add it again.",
                )
            })?;
            if upload.whole.is_none() {
                return Err(invalid("A photo has not arrived whole yet."));
            }
            if upload.sending {
                return Err(busy("A photo is being sent with another message."));
            }
        }
        let mut held = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(upload) = self.staged.get_mut(id)
                && let Some(bytes) = &upload.whole
            {
                upload.sending = true;
                held.push(Staged {
                    bytes: bytes.clone(),
                    name: upload.name.clone(),
                    mime: upload.mime,
                    _promise: upload.promise.clone(),
                });
            }
        }
        Ok(held)
    }

    /// A message with `ids` was not sent: they stay staged, and the clock starts again.
    fn release(&mut self, ids: &[String], now: Instant) {
        for id in ids {
            if let Some(upload) = self.staged.get_mut(id) {
                upload.sending = false;
                upload.touched = now;
            }
        }
    }

    /// Drops what nothing touched for [`EXPIRES`] by `now`, unless a message is being sent with
    /// it.
    pub(crate) fn expire(&mut self, now: Instant) {
        let old: Vec<String> = self
            .staged
            .iter()
            .filter(|(_, upload)| {
                !upload.sending && now.saturating_duration_since(upload.touched) >= EXPIRES
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in old {
            self.remove(&id);
        }
    }

    /// Drops everything: the session is over. A message being sent keeps what it holds, and its
    /// room with it.
    pub(crate) fn clear(&mut self) {
        let all: Vec<String> = self.staged.keys().cloned().collect();
        for id in all {
            self.remove(&id);
        }
    }

    /// Takes photo `id` off the stage. Its room goes back with the last copy of its bytes.
    fn remove(&mut self, id: &str) {
        self.staged.remove(id);
    }

    #[cfg(test)]
    fn bytes(&self) -> usize {
        self.bytes.load(Ordering::SeqCst)
    }
}

impl Drop for Uploads {
    fn drop(&mut self) {
        self.clear();
    }
}

/// Photos held for one message: dropped from the stage once [`Held::sent`] says it went, and
/// staged again otherwise, whatever ended the attempt.
pub(crate) struct Held {
    uploads: Arc<Mutex<Uploads>>,
    ids: Vec<String>,
    pub images: Vec<Staged>,
    done: bool,
}

impl Held {
    /// Holds the whole photos `ids` names for one message. None named, nothing held.
    pub(crate) fn take(
        uploads: &Arc<Mutex<Uploads>>,
        ids: &[String],
    ) -> Result<Self, BackendError> {
        let images = if ids.is_empty() {
            Vec::new()
        } else {
            lock(uploads).hold(ids)?
        };
        Ok(Self {
            uploads: uploads.clone(),
            ids: ids.to_vec(),
            images,
            done: false,
        })
    }

    /// The message went: its photos are no longer staged.
    pub(crate) fn sent(mut self) {
        let mut uploads = lock(&self.uploads);
        for id in &self.ids {
            uploads.remove(id);
        }
        drop(uploads);
        self.done = true;
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        if !self.done && !self.ids.is_empty() {
            lock(&self.uploads).release(&self.ids, Instant::now());
        }
    }
}

/// The photos of one message as the chat will keep them, measured before anything is made with
/// them: together they take at most [`MAX_PHOTOS`], what an agent's message may carry.
pub(crate) async fn prepare(
    backend: &Arc<dyn Backend>,
    images: &[Staged],
) -> Result<Vec<PreparedImage>, BackendError> {
    let mut prepared = Vec::with_capacity(images.len());
    let mut total = 0usize;
    for image in images {
        let image = backend
            .prepare_image(image.bytes.to_vec(), &image.name, image.mime)
            .await?;
        total = total.saturating_add(image.bytes.len());
        if total > MAX_PHOTOS {
            return Err(too_large(
                "These photos are more than 8 MB together. Send fewer at a time.",
            ));
        }
        prepared.push(image);
    }
    Ok(prepared)
}

/// Sends `text` to `chat_id` with `images`: each added to the chat's message box first, then
/// the message with them, as the window's message box sends one.
pub(crate) async fn send_with(
    backend: &Arc<dyn Backend>,
    chat_id: &str,
    text: String,
    images: &[PreparedImage],
) -> Result<PromptAck, BackendError> {
    let mut ids = Vec::with_capacity(images.len());
    for image in images {
        ids.push(
            backend
                .attach_image(chat_id, image.bytes.clone(), &image.name, &image.mime)
                .await?,
        );
    }
    backend.send_input(chat_id, text, ids).await
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    fn hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn piece(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    /// A photo arrives in order, piece by piece, is checked against its SHA-256 and is then
    /// whole; a piece out of order, too large or past the photo's size is refused, and one that
    /// does not match drops the photo.
    #[test]
    fn a_photo_arrives_in_order_and_is_checked() {
        let computer = Arc::new(AtomicUsize::new(0));
        let mut uploads = Uploads::new(computer.clone());
        let now = Instant::now();
        let photo: Vec<u8> = (0..UPLOAD_CHUNK + 10).map(|n| (n % 251) as u8).collect();
        let id = uploads
            .begin(
                "IMG_1.jpg",
                "image/jpeg",
                photo.len() as u64,
                &hex(&photo),
                now,
            )
            .expect("begun");
        assert_eq!(id.len(), 12, "unpadded base64url of 9 bytes");
        assert_eq!(computer.load(Ordering::SeqCst), photo.len());
        let (first, rest) = photo.split_at(UPLOAD_CHUNK);
        let refused = uploads.chunk(&id, 1, &piece(first), now).unwrap_err();
        assert_eq!(refused.code, "invalid", "out of order");
        assert_eq!(
            uploads.chunk(&id, 0, &piece(first), now).expect("first"),
            (UPLOAD_CHUNK, false)
        );
        let too_much = [rest, &[0u8; 5]].concat();
        assert_eq!(
            uploads
                .chunk(&id, 1, &piece(&too_much), now)
                .unwrap_err()
                .code,
            "too_large"
        );
        assert_eq!(
            uploads.chunk(&id, 1, "not base64!", now).unwrap_err().code,
            "invalid"
        );
        assert_eq!(
            uploads.chunk(&id, 1, &piece(rest), now).expect("last"),
            (photo.len(), true)
        );
        assert_eq!(
            uploads.chunk(&id, 2, &piece(rest), now).unwrap_err().code,
            "invalid",
            "whole already"
        );
        let held = uploads.hold(std::slice::from_ref(&id)).expect("held");
        assert_eq!(&*held[0].bytes, photo.as_slice());
        assert_eq!(held[0].mime, "image/jpeg");
        assert_eq!(held[0].name, "IMG_1.jpg");

        // Not as it was sent: dropped, its room given back.
        let wrong = uploads
            .begin("", "image/png", 3, &hex(b"abc"), now)
            .expect("begun");
        assert_eq!(
            uploads
                .chunk(&wrong, 0, &piece(b"abd"), now)
                .unwrap_err()
                .code,
            "corrupt"
        );
        assert_eq!(
            uploads
                .chunk(&wrong, 1, &piece(b"c"), now)
                .unwrap_err()
                .code,
            "not_found"
        );
        assert_eq!(computer.load(Ordering::SeqCst), photo.len());
    }

    /// What a session and the computer may hold, how many arrive at once, and what a photo may
    /// be; everything given back when the session ends.
    #[test]
    fn a_session_and_the_computer_hold_only_so_much() {
        let computer = Arc::new(AtomicUsize::new(0));
        let mut uploads = Uploads::new(computer.clone());
        let now = Instant::now();
        let sum = hex(b"x");
        for (mime, size, sha, code) in [
            ("image/heic", 1, sum.as_str(), "invalid"),
            ("image/png", 0, sum.as_str(), "invalid"),
            ("image/png", 1, "abc", "invalid"),
            ("image/png", 1, &"z".repeat(64), "invalid"),
            (
                "image/png",
                (MAX_UPLOAD + 1) as u64,
                sum.as_str(),
                "too_large",
            ),
        ] {
            assert_eq!(
                uploads.begin("a", mime, size, sha, now).unwrap_err().code,
                code,
                "{mime} {size}"
            );
        }
        // Four at once; a fifth waits.
        let open: Vec<String> = (0..MAX_OPEN)
            .map(|_| uploads.begin("a", "image/png", 1, &sum, now).expect("open"))
            .collect();
        assert_eq!(
            uploads
                .begin("a", "image/png", 1, &sum, now)
                .unwrap_err()
                .code,
            "busy"
        );
        for id in &open {
            assert!(uploads.chunk(id, 0, &piece(b"x"), now).expect("whole").1);
        }
        // Whole ones no longer count as arriving, but the session's room does.
        let big = uploads
            .begin("a", "image/png", MAX_UPLOAD as u64, &sum, now)
            .expect("8 MiB");
        uploads.abort(&big);
        for _ in 0..2 {
            uploads
                .begin("a", "image/png", MAX_UPLOAD as u64, &sum, now)
                .expect("room");
        }
        assert_eq!(
            uploads
                .begin("a", "image/png", MAX_UPLOAD as u64, &sum, now)
                .unwrap_err()
                .code,
            "busy",
            "24 MiB a session"
        );
        // The computer's room is shared by every session.
        let mut other = Uploads::new(computer.clone());
        for _ in 0..3 {
            other
                .begin("a", "image/png", MAX_UPLOAD as u64, &sum, now)
                .expect("another session's");
        }
        // Two sessions hold 40 MiB: a third has room for two more of 8 MiB, not three.
        let mut third = Uploads::new(computer.clone());
        for _ in 0..2 {
            third
                .begin("a", "image/png", MAX_UPLOAD as u64, &sum, now)
                .expect("the computer's room");
        }
        assert_eq!(
            third
                .begin("a", "image/png", MAX_UPLOAD as u64, &sum, now)
                .unwrap_err()
                .code,
            "busy",
            "64 MiB a computer"
        );
        assert!(third.begin("a", "image/png", 1, &sum, now).is_ok());
        drop(other);
        drop(third);
        uploads.clear();
        assert_eq!(uploads.bytes(), 0);
        assert_eq!(computer.load(Ordering::SeqCst), 0, "all given back");
    }

    /// A photo nothing touched for ten minutes goes, unless a message is being sent with it; one
    /// a message carried goes, and one a failed message held is staged again.
    #[test]
    fn staged_photos_expire_and_go_with_their_message() {
        let computer = Arc::new(AtomicUsize::new(0));
        let uploads = Arc::new(Mutex::new(Uploads::new(computer.clone())));
        let now = Instant::now();
        let sum = hex(b"x");
        let stage = || {
            let mut staged = lock(&uploads);
            let id = staged.begin("a", "image/png", 1, &sum, now).expect("begun");
            staged.chunk(&id, 0, &piece(b"x"), now).expect("whole");
            id
        };
        let (kept, held, gone) = (stage(), stage(), stage());
        let sending = Held::take(&uploads, std::slice::from_ref(&held)).expect("held");
        assert_eq!(
            Held::take(&uploads, std::slice::from_ref(&held))
                .err()
                .expect("twice")
                .code,
            "busy"
        );
        assert_eq!(
            Held::take(&uploads, &[kept.clone(), kept.clone()])
                .err()
                .expect("named twice")
                .code,
            "invalid"
        );
        lock(&uploads).expire(now + EXPIRES - Duration::from_secs(1));
        assert_eq!(lock(&uploads).staged.len(), 3, "not yet");
        lock(&uploads).expire(now + EXPIRES);
        assert_eq!(
            lock(&uploads).staged.keys().cloned().collect::<Vec<_>>(),
            std::slice::from_ref(&held),
            "only the one being sent stays"
        );
        drop(sending);
        assert!(!lock(&uploads).staged[&held].sending, "staged again");
        let again = Held::take(&uploads, std::slice::from_ref(&held)).expect("held again");
        again.sent();
        assert!(lock(&uploads).staged.is_empty());
        assert_eq!(computer.load(Ordering::SeqCst), 0);
        let _ = (kept, gone);
    }

    /// A photo's room goes back only when nothing holds its bytes: dropped from the stage, or the
    /// session over, while a message is being sent with it, it counts until that send lets go.
    #[test]
    fn a_photos_room_goes_back_with_its_last_holder() {
        let computer = Arc::new(AtomicUsize::new(0));
        let uploads = Arc::new(Mutex::new(Uploads::new(computer.clone())));
        let now = Instant::now();
        let stage = |bytes: &[u8]| {
            let mut staged = lock(&uploads);
            let id = staged
                .begin("a", "image/png", bytes.len() as u64, &hex(bytes), now)
                .expect("begun");
            staged.chunk(&id, 0, &piece(bytes), now).expect("whole");
            id
        };
        let (one, two) = (stage(b"first"), stage(b"second"));
        assert_eq!(computer.load(Ordering::SeqCst), 11);
        let sending = Held::take(&uploads, std::slice::from_ref(&one)).expect("held");
        lock(&uploads).abort(&one);
        assert_eq!(
            computer.load(Ordering::SeqCst),
            11,
            "its bytes are still being sent"
        );
        let other = Held::take(&uploads, std::slice::from_ref(&two)).expect("held");
        lock(&uploads).clear();
        assert_eq!(lock(&uploads).bytes(), 11, "the session's room too");
        drop(sending);
        assert_eq!(computer.load(Ordering::SeqCst), 6);
        drop(other);
        assert_eq!(computer.load(Ordering::SeqCst), 0);
        assert_eq!(lock(&uploads).bytes(), 0);
    }
}
