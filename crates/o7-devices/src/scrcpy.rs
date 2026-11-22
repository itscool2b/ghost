//! A device's screen and input, through scrcpy's server (Genymobile, Apache-2.0).
//!
//! The server is a small Java program that runs on the device as the shell user: it encodes the
//! screen as H.264 and injects input. o7 bundles one pinned release, pushes it to the device, and
//! starts it in forward-tunnel mode, where the server listens on a local socket on the device
//! (`localabstract:scrcpy_<id>`). Both of its sockets are opened through the adb transport, so
//! no TCP port is forwarded and nothing listens on this computer.
//!
//! The protocol belongs to scrcpy and changes between its versions: the client here speaks the
//! protocol of exactly [`VERSION`], and the server it talks to is always the bundled one.

use std::hash::{BuildHasher, Hasher};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::task::AbortHandle;

use crate::adb::Adb;
use crate::Error;

/// The scrcpy release whose server is bundled and whose protocol this module speaks.
pub const VERSION: &str = "5.0.1";
/// SHA-256 of the bundled server, as published with the release (`SHA256SUMS.txt`).
pub const SERVER_SHA256: &str = "764eb6f79811d5211fe9df341120882ba9994c7a61b897d7bf3fb662e53bc536";

const SERVER: &[u8] = include_bytes!("../resources/scrcpy-server-v5.0.1");
/// Where the server is put on the device: the folder the shell user may write and run from. The
/// name carries the version, so two o7 versions on one phone do not overwrite each other.
const REMOTE_PATH: &str = "/data/local/tmp/o7-scrcpy-server-5.0.1.jar";

/// The largest video packet accepted (a key frame at full size is well under 1 MiB).
const PACKET_LIMIT: usize = 16 * 1024 * 1024;
/// scrcpy's own limit for one text message.
const TEXT_LIMIT: usize = 300;
/// scrcpy's own limit for clipboard text (its 256 KiB message less the header).
const CLIPBOARD_LIMIT: usize = (1 << 18) - 14;

/// The finger o7 touches with: scrcpy's "generic finger", so the device sees a touch, not a mouse.
const FINGER: u64 = u64::MAX - 1;

/// How the screen is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// The longer side of the video, in pixels. The device's own shape is kept.
    pub max_size: u16,
    pub bit_rate: u32,
    pub max_fps: u8,
}

impl Default for Options {
    fn default() -> Self {
        Options { max_size: 1600, bit_rate: 8_000_000, max_fps: 60 }
    }
}

/// One thing read from the video socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Packet {
    /// The video's size from here on (at the start, and after each rotation).
    Size { width: u32, height: u32 },
    /// The decoder's configuration (H.264 SPS and PPS, in Annex B form).
    Config(Vec<u8>),
    /// One encoded picture.
    Frame {
        key: bool,
        /// Its time on the device's clock, in microseconds.
        pts: u64,
        data: Vec<u8>,
    },
}

/// The video socket.
#[derive(Debug)]
pub struct Video {
    stream: TcpStream,
}

/// The control socket: touches, keys and text to the device.
#[derive(Debug)]
pub struct Control {
    writer: OwnedWriteHalf,
    drain: AbortHandle,
    sequence: u64,
}

impl Drop for Control {
    fn drop(&mut self) {
        self.drain.abort();
    }
}

/// A running server and its two sockets. Dropping it closes them, and the server then exits and
/// puts back the settings it changed.
#[derive(Debug)]
pub struct Session {
    /// The device's own name for itself (`Pixel 8`).
    pub device_name: String,
    pub video: Video,
    pub control: Control,
    /// Keeps the server running; dropping it ends the server.
    pub server: Server,
}

/// The connection the server was started on. The server lives as long as this does.
#[derive(Debug)]
pub struct Server(AbortHandle);

impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// A touch's phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Touch {
    Down,
    Move,
    Up,
}

/// Android key codes (`KeyEvent.KEYCODE_*`) for the keys o7 sends.
pub mod key {
    pub const HOME: u32 = 3;
    pub const BACK: u32 = 4;
    pub const DPAD_UP: u32 = 19;
    pub const DPAD_DOWN: u32 = 20;
    pub const DPAD_LEFT: u32 = 21;
    pub const DPAD_RIGHT: u32 = 22;
    pub const VOLUME_UP: u32 = 24;
    pub const VOLUME_DOWN: u32 = 25;
    pub const POWER: u32 = 26;
    pub const TAB: u32 = 61;
    pub const SPACE: u32 = 62;
    pub const ENTER: u32 = 66;
    pub const DELETE_BACK: u32 = 67;
    pub const MENU: u32 = 82;
    pub const PAGE_UP: u32 = 92;
    pub const PAGE_DOWN: u32 = 93;
    pub const ESCAPE: u32 = 111;
    pub const DELETE_FORWARD: u32 = 112;
    pub const MOVE_HOME: u32 = 122;
    pub const MOVE_END: u32 = 123;
    pub const APP_SWITCH: u32 = 187;
}

/// Pushes the bundled server to `serial`, starts it and opens its sockets.
pub async fn start(adb: &Adb, serial: &str, options: Options) -> Result<Session, Error> {
    adb.push(serial, SERVER, REMOTE_PATH, 0o644).await?;
    // 31 random bits, as scrcpy's own client picks, so two sessions on one device do not meet.
    let scid = (std::collections::hash_map::RandomState::new().build_hasher().finish() as u32)
        & 0x7fff_ffff;
    let command = format!(
        "shell:CLASSPATH={REMOTE_PATH} app_process / com.genymobile.scrcpy.Server {VERSION} \
         scid={scid:08x} log_level=warn audio=false tunnel_forward=true max_size={} \
         video_bit_rate={} max_fps={} clipboard_autosync=false",
        options.max_size, options.bit_rate, options.max_fps
    );
    let mut server = adb.open(serial, &command).await?;
    let (said_tx, mut said_rx) = tokio::sync::watch::channel(String::new());
    // The server's own words (a refusal, a missing encoder) explain a failed start; after that
    // the connection is only held open, since closing it ends the server.
    let server = tokio::spawn(async move {
        let mut buffer = vec![0u8; 4096];
        let mut said = String::new();
        while let Ok(read) = server.read(&mut buffer).await {
            if read == 0 {
                break;
            }
            if said.len() < 4096 {
                said.push_str(&String::from_utf8_lossy(&buffer[..read]));
                let _ = said_tx.send(said.clone());
            }
        }
    })
    .abort_handle();

    let socket = format!("localabstract:scrcpy_{scid:08x}");
    let opened = tokio::time::timeout(Duration::from_secs(15), async {
        let mut video = open_when_listening(adb, serial, &socket).await?;
        // In forward mode the server sends one byte on its first socket, so a tunnel with
        // nothing behind it is told apart from a server that accepted.
        let mut dummy = [0u8; 1];
        video.read_exact(&mut dummy).await?;
        let control = adb.open(serial, &socket).await?;
        let mut name = [0u8; 64];
        video.read_exact(&mut name).await?;
        let mut codec = [0u8; 4];
        video.read_exact(&mut codec).await?;
        Ok::<_, Error>((video, control, name, codec))
    })
    .await;
    let (video, control, name, codec) = match opened {
        Ok(Ok(opened)) => opened,
        failed => {
            server.abort();
            let said = said_rx.borrow_and_update().clone();
            let said = said.lines().rev().find(|line| !line.trim().is_empty()).map(str::trim);
            return Err(match (failed, said) {
                (_, Some(said)) => {
                    Error::Device(format!("The device's screen could not be shown: {said}"))
                }
                (Ok(Err(error)), None) => error,
                (_, None) => Error::Timeout,
            });
        }
    };
    if &codec != b"h264" {
        server.abort();
        return Err(Error::Device(
            "The device could not encode its screen as H.264 video.".into(),
        ));
    }
    let end = name.iter().position(|byte| *byte == 0).unwrap_or(name.len());
    let device_name = String::from_utf8_lossy(&name[..end]).into_owned();
    let (mut reader, writer) = control.into_split();
    // What the device sends back (its clipboard, acknowledgements) is not used; it is read so
    // the socket never fills.
    let drain = tokio::spawn(async move {
        let mut buffer = vec![0u8; 8192];
        while matches!(reader.read(&mut buffer).await, Ok(read) if read > 0) {}
    })
    .abort_handle();
    Ok(Session {
        device_name,
        video: Video { stream: video },
        control: Control { writer, drain, sequence: 0 },
        server: Server(server),
    })
}

/// The server needs a moment before it listens; until then adb answers that the socket is closed.
async fn open_when_listening(adb: &Adb, serial: &str, socket: &str) -> Result<TcpStream, Error> {
    loop {
        match adb.open(serial, socket).await {
            Ok(stream) => return Ok(stream),
            Err(Error::Adb(_)) => tokio::time::sleep(Duration::from_millis(100)).await,
            Err(error) => return Err(error),
        }
    }
}

/// A packet's 12-byte header, decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Head {
    Size { width: u32, height: u32 },
    Media { config: bool, key: bool, pts: u64, length: usize },
}

fn parse_head(head: [u8; 12]) -> Head {
    let first = u64::from_be_bytes([
        head[0], head[1], head[2], head[3], head[4], head[5], head[6], head[7],
    ]);
    let second = u32::from_be_bytes([head[8], head[9], head[10], head[11]]);
    if first >> 63 == 1 {
        // A session packet: a flags word, then the width and the height.
        return Head::Size { width: (first & 0xffff_ffff) as u32, height: second };
    }
    let config = first & (1 << 62) != 0;
    Head::Media {
        config,
        key: first & (1 << 61) != 0,
        pts: if config { 0 } else { first & ((1 << 61) - 1) },
        length: second as usize,
    }
}

impl Video {
    /// The next packet. An error means the stream ended: the device went away, or the session
    /// was closed.
    pub async fn next(&mut self) -> Result<Packet, Error> {
        let mut head = [0u8; 12];
        self.stream.read_exact(&mut head).await.map_err(|_| Error::DeviceGone)?;
        match parse_head(head) {
            Head::Size { width, height } => Ok(Packet::Size { width, height }),
            Head::Media { config, key, pts, length } => {
                if length > PACKET_LIMIT {
                    return Err(Error::TooLarge);
                }
                let mut data = vec![0u8; length];
                self.stream.read_exact(&mut data).await.map_err(|_| Error::DeviceGone)?;
                Ok(if config { Packet::Config(data) } else { Packet::Frame { key, pts, data } })
            }
        }
    }
}

impl Control {
    async fn send(&mut self, message: &[u8]) -> Result<(), Error> {
        self.writer.write_all(message).await.map_err(|_| Error::DeviceGone)?;
        self.writer.flush().await.map_err(|_| Error::DeviceGone)
    }

    /// A finger at `(x, y)` in a video of `size` (the latest [`Packet::Size`]). The device
    /// ignores a touch made for another size, which is how a touch drawn on a picture from before
    /// a rotation is dropped.
    pub async fn touch(
        &mut self,
        phase: Touch,
        x: u32,
        y: u32,
        size: (u16, u16),
    ) -> Result<(), Error> {
        self.send(&touch_message(phase, x, y, size)).await
    }

    /// A wheel or two-finger scroll at `(x, y)`: `horizontal` and `vertical` are in notches,
    /// between -16 and 16.
    pub async fn scroll(
        &mut self,
        x: u32,
        y: u32,
        size: (u16, u16),
        horizontal: f32,
        vertical: f32,
    ) -> Result<(), Error> {
        self.send(&scroll_message(x, y, size, horizontal, vertical)).await
    }

    /// One key, pressed and released.
    pub async fn press(&mut self, keycode: u32) -> Result<(), Error> {
        self.send(&key_message(false, keycode, 0)).await?;
        self.send(&key_message(true, keycode, 0)).await
    }

    /// Selects everything in the field that has focus (Ctrl+A), so typing replaces it.
    pub async fn select_all(&mut self) -> Result<(), Error> {
        // KeyEvent.META_CTRL_ON | META_CTRL_LEFT_ON, and KEYCODE_A.
        const CTRL: u32 = 0x1000 | 0x2000;
        self.send(&key_message(false, 29, CTRL)).await?;
        self.send(&key_message(true, 29, CTRL)).await
    }

    /// Types `text` into whatever has focus. Plain ASCII goes as key events; anything else
    /// (accents, other scripts, emoji) has no key, so it is pasted through the clipboard.
    pub async fn text(&mut self, text: &str) -> Result<(), Error> {
        if text.is_empty() {
            return Ok(());
        }
        if text.len() > CLIPBOARD_LIMIT {
            return Err(Error::Refused("That is too much text to type at once.".into()));
        }
        if text.chars().all(|ch| ch == ' ' || ch.is_ascii_graphic()) {
            for part in text.as_bytes().chunks(TEXT_LIMIT) {
                self.send(&text_message(part)).await?;
            }
            return Ok(());
        }
        self.sequence += 1;
        self.send(&paste_message(self.sequence, text)).await
    }

    /// Back, or waking the screen when it is off.
    pub async fn back(&mut self) -> Result<(), Error> {
        self.send(&[BACK_OR_SCREEN_ON, 0]).await?;
        self.send(&[BACK_OR_SCREEN_ON, 1]).await
    }

    /// Turns the device a quarter turn.
    pub async fn rotate(&mut self) -> Result<(), Error> {
        self.send(&[ROTATE_DEVICE]).await
    }

    /// Asks the encoder to start again, which brings a fresh configuration and a key frame: what
    /// a viewer that has just arrived needs before it can draw.
    pub async fn reset_video(&mut self) -> Result<(), Error> {
        self.send(&[RESET_VIDEO]).await
    }
}

const INJECT_KEYCODE: u8 = 0;
const INJECT_TEXT: u8 = 1;
const INJECT_TOUCH_EVENT: u8 = 2;
const INJECT_SCROLL_EVENT: u8 = 3;
const BACK_OR_SCREEN_ON: u8 = 4;
const SET_CLIPBOARD: u8 = 9;
const ROTATE_DEVICE: u8 = 11;
const RESET_VIDEO: u8 = 17;

fn position(out: &mut Vec<u8>, x: u32, y: u32, size: (u16, u16)) {
    out.extend_from_slice(&x.to_be_bytes());
    out.extend_from_slice(&y.to_be_bytes());
    out.extend_from_slice(&size.0.to_be_bytes());
    out.extend_from_slice(&size.1.to_be_bytes());
}

fn touch_message(phase: Touch, x: u32, y: u32, size: (u16, u16)) -> Vec<u8> {
    let mut out = Vec::with_capacity(32);
    out.push(INJECT_TOUCH_EVENT);
    // MotionEvent.ACTION_DOWN, ACTION_UP, ACTION_MOVE.
    out.push(match phase {
        Touch::Down => 0,
        Touch::Up => 1,
        Touch::Move => 2,
    });
    out.extend_from_slice(&FINGER.to_be_bytes());
    position(&mut out, x, y, size);
    // Pressure, as a 16-bit fraction: full while the finger is down.
    out.extend_from_slice(&(if phase == Touch::Up { 0u16 } else { 0xffff }).to_be_bytes());
    // The action button and the buttons held: none, since a finger has no buttons.
    out.extend_from_slice(&[0; 8]);
    out
}

/// A scroll amount in [-16, 16] as scrcpy's signed 16-bit fraction.
fn scroll_amount(notches: f32) -> i16 {
    let scaled = (f64::from(notches.clamp(-16.0, 16.0)) / 16.0 * 32768.0).round();
    scaled.clamp(-32768.0, 32767.0) as i16
}

fn scroll_message(x: u32, y: u32, size: (u16, u16), horizontal: f32, vertical: f32) -> Vec<u8> {
    let mut out = Vec::with_capacity(21);
    out.push(INJECT_SCROLL_EVENT);
    position(&mut out, x, y, size);
    out.extend_from_slice(&scroll_amount(horizontal).to_be_bytes());
    out.extend_from_slice(&scroll_amount(vertical).to_be_bytes());
    out.extend_from_slice(&[0; 4]);
    out
}

fn key_message(up: bool, keycode: u32, modifiers: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(14);
    out.push(INJECT_KEYCODE);
    out.push(u8::from(up));
    out.extend_from_slice(&keycode.to_be_bytes());
    // No repeat.
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&modifiers.to_be_bytes());
    out
}

fn text_message(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + text.len());
    out.push(INJECT_TEXT);
    out.extend_from_slice(&(text.len() as u32).to_be_bytes());
    out.extend_from_slice(text);
    out
}

fn paste_message(sequence: u64, text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(14 + text.len());
    out.push(SET_CLIPBOARD);
    out.extend_from_slice(&sequence.to_be_bytes());
    // Paste it once it is set.
    out.push(1);
    out.extend_from_slice(&(text.len() as u32).to_be_bytes());
    out.extend_from_slice(text.as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::*;

    #[test]
    fn the_bundled_server_is_the_pinned_release() {
        let digest = Sha256::digest(SERVER);
        let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(hex, SERVER_SHA256);
        assert!(REMOTE_PATH.contains(VERSION));
        // A jar is a zip archive.
        assert_eq!(&SERVER[..2], b"PK");
    }

    // The byte layouts below are scrcpy's own test vectors
    // (app/tests/test_control_msg_serialize.c at v5.0.1), with a finger in place of its mouse.

    #[test]
    fn a_touch_is_32_bytes_in_scrcpys_layout() {
        let message = touch_message(Touch::Down, 100, 200, (1080, 1920));
        assert_eq!(
            message,
            [
                2, 0, // inject touch, ACTION_DOWN
                0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe, // the generic finger
                0, 0, 0, 0x64, 0, 0, 0, 0xc8, // 100 200
                0x04, 0x38, 0x07, 0x80, // 1080 1920
                0xff, 0xff, // pressure
                0, 0, 0, 0, 0, 0, 0, 0, // no buttons
            ]
        );
        let up = touch_message(Touch::Up, 100, 200, (1080, 1920));
        assert_eq!(up[1], 1);
        assert_eq!(&up[22..24], [0, 0]);
        assert_eq!(touch_message(Touch::Move, 1, 2, (3, 4))[1], 2);
    }

    #[test]
    fn a_scroll_encodes_notches_as_signed_fractions() {
        let message = scroll_message(260, 1026, (1080, 1920), 16.0, -16.0);
        assert_eq!(
            message,
            [
                3, 0, 0, 0x01, 0x04, 0, 0, 0x04, 0x02, // 260 1026
                0x04, 0x38, 0x07, 0x80, // 1080 1920
                0x7f, 0xff, // 16
                0x80, 0x00, // -16
                0, 0, 0, 0,
            ]
        );
        assert_eq!(scroll_amount(0.0), 0);
        assert_eq!(scroll_amount(1.0), 2048);
        assert_eq!(scroll_amount(400.0), 0x7fff);
    }

    #[test]
    fn keys_text_and_paste_match_scrcpys_layout() {
        assert_eq!(key_message(true, key::ENTER, 0), [0, 1, 0, 0, 0, 0x42, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(key_message(false, key::BACK, 0)[1], 0);
        // scrcpy's vector: Enter up with Shift held.
        assert_eq!(key_message(true, key::ENTER, 0x41), [0, 1, 0, 0, 0, 0x42, 0, 0, 0, 0, 0, 0, 0, 0x41]);
        let mut text = vec![1, 0, 0, 0, 0x0d];
        text.extend_from_slice(b"hello, world!");
        assert_eq!(text_message(b"hello, world!"), text);
        let mut paste = vec![9, 1, 2, 3, 4, 5, 6, 7, 8, 1, 0, 0, 0, 0x0d];
        paste.extend_from_slice(b"hello, world!");
        assert_eq!(paste_message(0x0102_0304_0506_0708, "hello, world!"), paste);
    }

    #[test]
    fn headers_tell_sizes_configuration_and_frames_apart() {
        // A session packet: the first bit, then the width and the height.
        assert_eq!(
            parse_head([0x80, 0, 0, 0, 0, 0, 0x02, 0x40, 0, 0, 0x05, 0]),
            Head::Size { width: 576, height: 1280 }
        );
        // The "client resized" flag does not change the width.
        assert_eq!(
            parse_head([0x80, 0, 0, 1, 0, 0, 0x02, 0x40, 0, 0, 0x05, 0]),
            Head::Size { width: 576, height: 1280 }
        );
        assert_eq!(
            parse_head([0x40, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 32]),
            Head::Media { config: true, key: false, pts: 0, length: 32 }
        );
        assert_eq!(
            parse_head([0x20, 0, 0, 0, 0, 0, 0x03, 0xe8, 0, 0, 0x64, 0x41]),
            Head::Media { config: false, key: true, pts: 1000, length: 25665 }
        );
        assert_eq!(
            parse_head([0, 0, 0, 0, 0, 0x01, 0x86, 0xa0, 0, 0, 0, 9]),
            Head::Media { config: false, key: false, pts: 100_000, length: 9 }
        );
    }
}
