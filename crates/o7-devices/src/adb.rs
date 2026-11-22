//! adb's own wire protocol, spoken to the adb server on this computer (127.0.0.1:5037).
//!
//! No `adb` process is started per call: a request is one TCP connection to the server, a
//! `host:transport:<serial>` line that points it at a device, then a service (`exec:`, `shell:`,
//! `sync:`, `localabstract:`). The server is started with `adb start-server` when nothing answers.
//! Every connection is to the loopback address; nothing here listens, and nothing forwards a
//! device's port to this computer.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::Path;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::Error;

/// The adb server's port unless `ANDROID_ADB_SERVER_PORT` names another.
pub const DEFAULT_PORT: u16 = 5037;

const CONNECT: Duration = Duration::from_secs(3);
/// The longest service line sent. Older servers refuse more than 1024 bytes.
const SERVICE_LIMIT: usize = 1024;
/// One `DATA` chunk of a file being pushed.
const SYNC_CHUNK: usize = 64 * 1024;

/// The adb server, by address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adb {
    addr: SocketAddr,
}

impl Default for Adb {
    fn default() -> Self {
        Adb::at(DEFAULT_PORT)
    }
}

/// How the server describes a device's connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceState {
    /// Ready for commands.
    Device,
    /// Plugged in, but the person has not allowed this computer on the phone yet.
    Unauthorized,
    /// Known but not answering (still booting, or the cable is going).
    Offline,
    /// Anything else the server says (`authorizing`, `recovery`, `no permissions ...`).
    Other(String),
}

/// One line of `adb devices -l`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub serial: String,
    pub state: DeviceState,
    /// The phone's model, with spaces as the server writes them (`Pixel_8`).
    pub model: Option<String>,
}

impl Listed {
    /// Whether the serial is an emulator's (`emulator-5554`), and its console port.
    pub fn emulator_port(&self) -> Option<u16> {
        emulator_port(&self.serial)
    }
}

/// The console port in an emulator's serial (`emulator-5554` is 5554).
pub fn emulator_port(serial: &str) -> Option<u16> {
    serial.strip_prefix("emulator-")?.parse().ok()
}

/// What a command printed and how it ended.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShellOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit: u8,
}

impl ShellOutput {
    /// What it printed, trimmed.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).trim().to_owned()
    }

    /// Its output when it succeeded; otherwise an error that says what `doing` failed and why.
    pub fn ok(self, doing: &str) -> Result<String, Error> {
        if self.exit == 0 {
            return Ok(self.text());
        }
        let stderr = String::from_utf8_lossy(&self.stderr);
        let said = if stderr.trim().is_empty() { self.text() } else { stderr.trim().to_owned() };
        let said = said.lines().take(6).collect::<Vec<_>>().join(" ");
        Err(Error::Device(if said.is_empty() {
            format!("{doing} failed on the device (exit {}).", self.exit)
        } else {
            format!("{doing} failed on the device: {said}")
        }))
    }
}

/// A live list of devices: each [`Tracker::next`] waits for the list to change.
#[derive(Debug)]
pub struct Tracker {
    stream: TcpStream,
}

impl Tracker {
    /// The next list. An error means the server went away; start it and track again.
    pub async fn next(&mut self) -> Result<Vec<Listed>, Error> {
        let block = read_block(&mut self.stream, 1024 * 1024).await?;
        Ok(parse_devices(&String::from_utf8_lossy(&block)))
    }
}

impl Adb {
    /// The server on this computer at `port`.
    pub fn at(port: u16) -> Adb {
        Adb { addr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)) }
    }

    /// The server where adb itself would look: `ANDROID_ADB_SERVER_PORT`, else 5037.
    pub fn from_env() -> Adb {
        let port = std::env::var("ANDROID_ADB_SERVER_PORT")
            .ok()
            .and_then(|value| value.trim().parse::<u16>().ok())
            .filter(|port| *port != 0)
            .unwrap_or(DEFAULT_PORT);
        Adb::at(port)
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    async fn connect(&self) -> Result<TcpStream, Error> {
        match tokio::time::timeout(CONNECT, TcpStream::connect(self.addr)).await {
            Ok(Ok(stream)) => {
                let _ = stream.set_nodelay(true);
                Ok(stream)
            }
            Ok(Err(error)) => Err(Error::AdbUnreachable(error.to_string())),
            Err(_) => Err(Error::AdbUnreachable("it did not answer".into())),
        }
    }

    /// The server's protocol version, which also says it is there.
    pub async fn version(&self) -> Result<u32, Error> {
        let mut stream = self.connect().await?;
        request(&mut stream, "host:version").await?;
        let block = read_block(&mut stream, 16).await?;
        std::str::from_utf8(&block)
            .ok()
            .and_then(|text| u32::from_str_radix(text, 16).ok())
            .ok_or_else(|| Error::Protocol("adb answered a version that is not a number".into()))
    }

    /// Makes sure a server answers, starting `program start-server` when none does.
    pub async fn ensure(&self, program: &Path) -> Result<(), Error> {
        if self.version().await.is_ok() {
            return Ok(());
        }
        let mut command = crate::command(program);
        command.arg("-P").arg(self.port().to_string()).arg("start-server");
        let started = tokio::time::timeout(Duration::from_secs(20), command.output()).await;
        match started {
            Ok(Ok(output)) if output.status.success() => {}
            Ok(Ok(output)) => {
                let said = String::from_utf8_lossy(&output.stderr);
                return Err(Error::AdbUnreachable(format!(
                    "adb could not start its server: {}",
                    said.lines().last().unwrap_or("no reason given").trim()
                )));
            }
            Ok(Err(error)) => {
                return Err(Error::AdbUnreachable(format!("adb could not be run: {error}")));
            }
            Err(_) => return Err(Error::AdbUnreachable("adb took too long to start".into())),
        }
        self.version().await.map(drop)
    }

    /// Every device the server knows.
    pub async fn devices(&self) -> Result<Vec<Listed>, Error> {
        let mut stream = self.connect().await?;
        request(&mut stream, "host:devices-l").await?;
        let block = read_block(&mut stream, 1024 * 1024).await?;
        Ok(parse_devices(&String::from_utf8_lossy(&block)))
    }

    /// The device list now and after each change.
    pub async fn track(&self) -> Result<Tracker, Error> {
        let mut stream = self.connect().await?;
        request(&mut stream, "host:track-devices-l").await?;
        Ok(Tracker { stream })
    }

    /// A connection to `service` on the device `serial`.
    pub async fn open(&self, serial: &str, service: &str) -> Result<TcpStream, Error> {
        let serial = checked_serial(serial)?;
        let mut stream = self.connect().await?;
        request(&mut stream, &format!("host:transport:{serial}")).await?;
        request(&mut stream, service).await?;
        Ok(stream)
    }

    /// Runs `command` in the device's shell and returns its standard output, byte for byte (a
    /// picture, a dump). At most `limit` bytes, within `within`.
    pub async fn exec(
        &self,
        serial: &str,
        command: &str,
        limit: usize,
        within: Duration,
    ) -> Result<Vec<u8>, Error> {
        let work = async {
            let mut stream = self.open(serial, &format!("exec:{command}")).await?;
            read_to_end(&mut stream, limit).await
        };
        tokio::time::timeout(within, work).await.map_err(|_| Error::Timeout)?
    }

    /// Runs `command` in the device's shell and returns what it printed, what it complained
    /// about and its exit code.
    pub async fn shell(
        &self,
        serial: &str,
        command: &str,
        limit: usize,
        within: Duration,
    ) -> Result<ShellOutput, Error> {
        let work = async {
            let mut stream = self.open(serial, &format!("shell,v2,raw:{command}")).await?;
            read_shell(&mut stream, limit).await
        };
        tokio::time::timeout(within, work).await.map_err(|_| Error::Timeout)?
    }

    /// Writes `data` to the file `remote` on the device, with Unix permissions `mode`.
    pub async fn push(
        &self,
        serial: &str,
        data: &[u8],
        remote: &str,
        mode: u32,
    ) -> Result<(), Error> {
        if remote.contains(',') || remote.contains('\0') || !remote.starts_with('/') {
            return Err(Error::Refused("A device path must be absolute, with no comma.".into()));
        }
        let mut stream = self.open(serial, "sync:").await?;
        let spec = format!("{remote},{}", mode & 0o777);
        let mut head = Vec::with_capacity(8 + spec.len());
        head.extend_from_slice(b"SEND");
        head.extend_from_slice(&u32_le(spec.len())?);
        head.extend_from_slice(spec.as_bytes());
        stream.write_all(&head).await?;
        for chunk in data.chunks(SYNC_CHUNK) {
            stream.write_all(b"DATA").await?;
            stream.write_all(&u32_le(chunk.len())?).await?;
            stream.write_all(chunk).await?;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        stream.write_all(b"DONE").await?;
        stream.write_all(&u32::try_from(now).unwrap_or(u32::MAX).to_le_bytes()).await?;
        stream.flush().await?;
        let mut reply = [0u8; 8];
        stream.read_exact(&mut reply).await?;
        if &reply[..4] == b"OKAY" {
            return Ok(());
        }
        let length = u32::from_le_bytes([reply[4], reply[5], reply[6], reply[7]]) as usize;
        let mut message = vec![0u8; length.min(4096)];
        stream.read_exact(&mut message).await?;
        Err(Error::Device(format!(
            "The device refused the file: {}",
            String::from_utf8_lossy(&message).trim()
        )))
    }

    /// Installs the APK at `apk`, replacing an installed copy. Its bytes are streamed to the
    /// device's package manager, so nothing is left behind in the device's storage.
    pub async fn install(&self, serial: &str, apk: &Path, within: Duration) -> Result<(), Error> {
        let work = async {
            let mut file = tokio::fs::File::open(apk).await.map_err(|error| {
                Error::Refused(format!("The app file could not be opened: {error}"))
            })?;
            let size = file.metadata().await?.len();
            if size == 0 {
                return Err(Error::Refused("The app file is empty.".into()));
            }
            let service = format!("exec:cmd package install -r -t -d -S {size}");
            let mut stream = self.open(serial, &service).await?;
            tokio::io::copy(&mut file, &mut stream).await?;
            stream.flush().await?;
            let reply = read_to_end(&mut stream, 64 * 1024).await?;
            let text = String::from_utf8_lossy(&reply);
            if text.trim_start().starts_with("Success") {
                Ok(())
            } else {
                Err(Error::Device(install_failure(&text)))
            }
        };
        tokio::time::timeout(within, work).await.map_err(|_| Error::Timeout)?
    }

    /// Lets the device reach this computer's `host_port` as its own `device_port` on localhost (a
    /// development server). The listener is on the device, not on this computer.
    pub async fn reverse(
        &self,
        serial: &str,
        device_port: u16,
        host_port: u16,
    ) -> Result<(), Error> {
        let service = format!("reverse:forward:tcp:{device_port};tcp:{host_port}");
        let mut stream = self.open(serial, &service).await?;
        read_status(&mut stream).await
    }

    /// Ends [`Adb::reverse`] for `device_port`.
    pub async fn reverse_remove(&self, serial: &str, device_port: u16) -> Result<(), Error> {
        let service = format!("reverse:killforward:tcp:{device_port}");
        let mut stream = self.open(serial, &service).await?;
        read_status(&mut stream).await
    }
}

/// A serial as the server prints them: letters, digits and `. : - _` (an emulator, a USB serial,
/// an address, an mDNS name). Anything else is refused, so a serial can sit inside a service line.
pub fn checked_serial(serial: &str) -> Result<&str, Error> {
    let valid = !serial.is_empty()
        && serial.len() <= 128
        && serial
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | ':' | '-' | '_'));
    if valid {
        Ok(serial)
    } else {
        Err(Error::Refused("That is not a device serial.".into()))
    }
}

fn u32_le(length: usize) -> Result<[u8; 4], Error> {
    u32::try_from(length)
        .map(u32::to_le_bytes)
        .map_err(|_| Error::Refused("That is too large to send to the device.".into()))
}

/// Sends one service line and reads the server's verdict.
async fn request<S>(stream: &mut S, service: &str) -> Result<(), Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if service.len() > SERVICE_LIMIT {
        return Err(Error::Refused("That command is too long for the device.".into()));
    }
    let mut line = format!("{:04x}", service.len()).into_bytes();
    line.extend_from_slice(service.as_bytes());
    stream.write_all(&line).await?;
    stream.flush().await?;
    read_status(stream).await
}

/// `OKAY`, or `FAIL` with its reason.
async fn read_status<S>(stream: &mut S) -> Result<(), Error>
where
    S: AsyncRead + Unpin,
{
    let mut status = [0u8; 4];
    stream.read_exact(&mut status).await.map_err(closed_early)?;
    match &status {
        b"OKAY" => Ok(()),
        b"FAIL" => {
            let reason = read_block(stream, 4096).await?;
            Err(refusal(String::from_utf8_lossy(&reason).trim()))
        }
        _ => Err(Error::Protocol("adb answered something that is not adb's protocol".into())),
    }
}

/// What the server's `FAIL` means for the person.
fn refusal(reason: &str) -> Error {
    let lower = reason.to_ascii_lowercase();
    if lower.contains("unauthorized") {
        Error::Unauthorized
    } else if lower.contains("not found") || lower.contains("offline") || lower.contains("no devices")
    {
        Error::DeviceGone
    } else {
        Error::Adb(reason.to_owned())
    }
}

fn closed_early(error: std::io::Error) -> Error {
    if error.kind() == std::io::ErrorKind::UnexpectedEof {
        Error::Protocol("adb closed the connection before it answered".into())
    } else {
        Error::Io(error)
    }
}

/// A length in four hex digits, then that many bytes.
async fn read_block<S>(stream: &mut S, limit: usize) -> Result<Vec<u8>, Error>
where
    S: AsyncRead + Unpin,
{
    let mut head = [0u8; 4];
    stream.read_exact(&mut head).await.map_err(closed_early)?;
    let length = std::str::from_utf8(&head)
        .ok()
        .and_then(|text| usize::from_str_radix(text, 16).ok())
        .ok_or_else(|| Error::Protocol("adb sent a length that is not a number".into()))?;
    if length > limit {
        return Err(Error::TooLarge);
    }
    let mut block = vec![0u8; length];
    stream.read_exact(&mut block).await.map_err(closed_early)?;
    Ok(block)
}

/// Whether a read error is only the other side hanging up, which adb's server does without a
/// clean close on some systems once a command has ended.
fn hung_up(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::BrokenPipe
    )
}

/// Everything until the other side closes, refusing more than `limit` bytes.
async fn read_to_end<S>(stream: &mut S, limit: usize) -> Result<Vec<u8>, Error>
where
    S: AsyncRead + Unpin,
{
    let mut out = Vec::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        match stream.read(&mut buffer).await {
            Ok(0) => return Ok(out),
            Ok(read) => {
                if out.len() + read > limit {
                    return Err(Error::TooLarge);
                }
                out.extend_from_slice(&buffer[..read]);
            }
            Err(error) if hung_up(&error) => return Ok(out),
            Err(error) => return Err(Error::Io(error)),
        }
    }
}

/// The shell protocol's packets: a kind, a little-endian length, the bytes.
async fn read_shell<S>(stream: &mut S, limit: usize) -> Result<ShellOutput, Error>
where
    S: AsyncRead + Unpin,
{
    const STDOUT: u8 = 1;
    const STDERR: u8 = 2;
    const EXIT: u8 = 3;
    let early = || Error::Protocol("the device closed the command before it finished".into());
    let mut output = ShellOutput::default();
    loop {
        let mut head = [0u8; 5];
        match stream.read_exact(&mut head).await {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof || hung_up(&error) => {
                return Err(early());
            }
            Err(error) => return Err(Error::Io(error)),
        }
        let length = u32::from_le_bytes([head[1], head[2], head[3], head[4]]) as usize;
        if output.stdout.len() + output.stderr.len() + length > limit {
            return Err(Error::TooLarge);
        }
        let mut data = vec![0u8; length];
        stream.read_exact(&mut data).await.map_err(|_| early())?;
        match head[0] {
            STDOUT => output.stdout.extend_from_slice(&data),
            STDERR => output.stderr.extend_from_slice(&data),
            EXIT => {
                output.exit = data.first().copied().unwrap_or(1);
                return Ok(output);
            }
            _ => {}
        }
    }
}

/// `adb devices -l`, parsed: a serial, a state, then `key:value` words.
fn parse_devices(text: &str) -> Vec<Listed> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let serial = words.next()?.to_owned();
            let mut state = Vec::new();
            let mut model = None;
            for word in words {
                match word.split_once(':') {
                    Some(("model", value)) => model = Some(value.to_owned()),
                    Some(("product" | "device" | "transport_id" | "usb", _)) => {}
                    _ => state.push(word),
                }
            }
            let state = match state.join(" ").as_str() {
                "device" => DeviceState::Device,
                "unauthorized" => DeviceState::Unauthorized,
                "offline" => DeviceState::Offline,
                other => DeviceState::Other(other.to_owned()),
            };
            Some(Listed { serial, state, model })
        })
        .collect()
}

/// The package manager's refusal, in plain words, with its own code kept at the end.
fn install_failure(reply: &str) -> String {
    let line = reply
        .lines()
        .find(|line| line.contains("Failure") || line.contains("Exception") || line.contains("rror"))
        .or_else(|| reply.lines().find(|line| !line.trim().is_empty()))
        .unwrap_or("no reason given")
        .trim();
    let line: String = line.chars().take(300).collect();
    let why = if line.contains("INSTALL_FAILED_UPDATE_INCOMPATIBLE")
        || line.contains("INSTALL_FAILED_SHARED_USER_INCOMPATIBLE")
    {
        "A copy signed with a different key is already installed. Uninstall it from the device first."
    } else if line.contains("INSTALL_FAILED_VERSION_DOWNGRADE") {
        "A newer version is already installed. Uninstall it from the device first."
    } else if line.contains("INSTALL_FAILED_INSUFFICIENT_STORAGE") {
        "The device has no room for it."
    } else if line.contains("INSTALL_FAILED_NO_MATCHING_ABIS") {
        "It was built for a different processor than this device has."
    } else if line.contains("INSTALL_FAILED_OLDER_SDK") {
        "It needs a newer Android than this device runs."
    } else if line.contains("INSTALL_PARSE_FAILED") || line.contains("INSTALL_FAILED_INVALID_APK") {
        "The file is not an app the device can read."
    } else {
        "The device did not install the app."
    };
    format!("{why} ({line})")
}

#[cfg(test)]
mod tests {
    use std::future::Future;

    use tokio::net::TcpListener;

    use super::*;

    /// A stand-in adb server: `serve` gets each connection in turn.
    async fn fake<F, Fut>(serve: F) -> Adb
    where
        F: Fn(TcpStream) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send,
    {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                serve(stream).await;
            }
        });
        Adb::at(port)
    }

    /// Reads one service line as a server does.
    async fn service(stream: &mut TcpStream) -> String {
        let mut head = [0u8; 4];
        stream.read_exact(&mut head).await.unwrap();
        let length = usize::from_str_radix(std::str::from_utf8(&head).unwrap(), 16).unwrap();
        let mut body = vec![0u8; length];
        stream.read_exact(&mut body).await.unwrap();
        String::from_utf8(body).unwrap()
    }

    async fn block(stream: &mut TcpStream, text: &str) {
        stream.write_all(format!("{:04x}{text}", text.len()).as_bytes()).await.unwrap();
    }

    const LONG: Duration = Duration::from_secs(5);

    #[tokio::test]
    async fn the_version_is_read_from_a_hex_block() {
        let adb = fake(|mut stream| async move {
            assert_eq!(service(&mut stream).await, "host:version");
            stream.write_all(b"OKAY").await.unwrap();
            block(&mut stream, "0029").await;
        })
        .await;
        assert_eq!(adb.version().await.unwrap(), 0x29);
    }

    #[tokio::test]
    async fn nothing_listening_is_unreachable_not_a_panic() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        assert!(matches!(Adb::at(port).version().await, Err(Error::AdbUnreachable(_))));
    }

    #[test]
    fn device_lines_keep_serial_state_and_model() {
        let listed = parse_devices(
            "emulator-5580          device product:sdk_gphone64_x86_64 model:sdk_gphone64_x86_64 device:emu64xa transport_id:1\n\
             R58M12ABCDE            unauthorized usb:1-4 transport_id:7\n\
             192.168.1.20:5555      offline\n\
             0123456789ABCDEF       no permissions (user in plugdev group; are your udev rules wrong?); see [http://developer.android.com/tools/device.html] usb:1-2\n\n",
        );
        assert_eq!(listed.len(), 4);
        assert_eq!(listed[0].serial, "emulator-5580");
        assert_eq!(listed[0].state, DeviceState::Device);
        assert_eq!(listed[0].model.as_deref(), Some("sdk_gphone64_x86_64"));
        assert_eq!(listed[0].emulator_port(), Some(5580));
        assert_eq!(listed[1].state, DeviceState::Unauthorized);
        assert_eq!(listed[1].emulator_port(), None);
        assert_eq!(listed[2].state, DeviceState::Offline);
        assert!(
            matches!(&listed[3].state, DeviceState::Other(text) if text.starts_with("no permissions"))
        );
    }

    #[tokio::test]
    async fn exec_goes_through_the_transport_and_returns_raw_bytes() {
        let adb = fake(|mut stream| async move {
            assert_eq!(service(&mut stream).await, "host:transport:emulator-5580");
            stream.write_all(b"OKAY").await.unwrap();
            assert_eq!(service(&mut stream).await, "exec:screencap -p");
            stream.write_all(b"OKAY").await.unwrap();
            stream.write_all(&[0x89, b'P', b'N', b'G', 0, 255, 10, 13]).await.unwrap();
        })
        .await;
        let bytes = adb.exec("emulator-5580", "screencap -p", 1024, LONG).await.unwrap();
        assert_eq!(bytes, [0x89, b'P', b'N', b'G', 0, 255, 10, 13]);
    }

    #[tokio::test]
    async fn exec_refuses_more_than_its_limit() {
        let adb = fake(|mut stream| async move {
            service(&mut stream).await;
            stream.write_all(b"OKAY").await.unwrap();
            service(&mut stream).await;
            stream.write_all(b"OKAY").await.unwrap();
            let _ = stream.write_all(&[7u8; 4096]).await;
        })
        .await;
        assert!(matches!(adb.exec("abc", "cat big", 100, LONG).await, Err(Error::TooLarge)));
    }

    #[tokio::test]
    async fn a_missing_or_unauthorized_device_says_so() {
        let adb = fake(|mut stream| async move {
            let asked = service(&mut stream).await;
            stream.write_all(b"FAIL").await.unwrap();
            if asked.ends_with("gone") {
                block(&mut stream, "device 'gone' not found").await;
            } else if asked.ends_with("locked") {
                block(&mut stream, "device unauthorized.\nThis adb server's $ADB_VENDOR_KEYS is not set").await;
            } else {
                block(&mut stream, "closed").await;
            }
        })
        .await;
        assert!(matches!(adb.exec("gone", "id", 10, LONG).await, Err(Error::DeviceGone)));
        assert!(matches!(adb.exec("locked", "id", 10, LONG).await, Err(Error::Unauthorized)));
        assert!(matches!(adb.exec("other", "id", 10, LONG).await, Err(Error::Adb(reason)) if reason == "closed"));
    }

    #[tokio::test]
    async fn serials_and_long_commands_are_refused_before_anything_is_sent() {
        let adb = fake(|_stream| async move { panic!("nothing should connect") }).await;
        for serial in ["", "a b", "x;host:kill", "emulator-5554\n", "a/b"] {
            assert!(matches!(adb.exec(serial, "id", 10, LONG).await, Err(Error::Refused(_))), "{serial:?}");
        }
        let adb = fake(|mut stream| async move {
            service(&mut stream).await;
            let _ = stream.write_all(b"OKAY").await;
        })
        .await;
        let long = "x".repeat(2000);
        assert!(matches!(adb.exec("abc", &long, 10, LONG).await, Err(Error::Refused(_))));
    }

    #[tokio::test]
    async fn shell_separates_output_errors_and_the_exit_code() {
        let adb = fake(|mut stream| async move {
            service(&mut stream).await;
            stream.write_all(b"OKAY").await.unwrap();
            assert_eq!(service(&mut stream).await, "shell,v2,raw:pm path com.example");
            stream.write_all(b"OKAY").await.unwrap();
            for (kind, data) in [(1u8, &b"package:/data/app/base.apk\n"[..]), (2, b"warn\n"), (9, b"??"), (3, &[4])] {
                stream.write_all(&[kind]).await.unwrap();
                stream.write_all(&(data.len() as u32).to_le_bytes()).await.unwrap();
                stream.write_all(data).await.unwrap();
            }
        })
        .await;
        let output = adb.shell("abc", "pm path com.example", 4096, LONG).await.unwrap();
        assert_eq!(output.text(), "package:/data/app/base.apk");
        assert_eq!(output.stderr, b"warn\n");
        assert_eq!(output.exit, 4);
        let error = output.ok("Finding the app").unwrap_err();
        assert_eq!(error.to_string(), "Finding the app failed on the device: warn");
    }

    #[tokio::test]
    async fn a_shell_that_ends_without_an_exit_code_is_an_error() {
        let adb = fake(|mut stream| async move {
            service(&mut stream).await;
            stream.write_all(b"OKAY").await.unwrap();
            service(&mut stream).await;
            stream.write_all(b"OKAY").await.unwrap();
            stream.write_all(&[1, 2, 0, 0, 0, b'h', b'i']).await.unwrap();
        })
        .await;
        assert!(matches!(adb.shell("abc", "id", 4096, LONG).await, Err(Error::Protocol(_))));
    }

    #[tokio::test]
    async fn push_sends_the_file_in_sync_chunks() {
        let (seen_tx, seen_rx) = tokio::sync::oneshot::channel::<Vec<u8>>();
        let seen_tx = std::sync::Mutex::new(Some(seen_tx));
        let adb = fake(move |mut stream| {
            let seen_tx = seen_tx.lock().unwrap().take();
            async move {
                service(&mut stream).await;
                stream.write_all(b"OKAY").await.unwrap();
                assert_eq!(service(&mut stream).await, "sync:");
                stream.write_all(b"OKAY").await.unwrap();
                let mut received = Vec::new();
                let mut file = Vec::new();
                loop {
                    let mut head = [0u8; 8];
                    stream.read_exact(&mut head).await.unwrap();
                    let length = u32::from_le_bytes(head[4..].try_into().unwrap()) as usize;
                    received.extend_from_slice(&head[..4]);
                    match &head[..4] {
                        b"SEND" => {
                            let mut spec = vec![0u8; length];
                            stream.read_exact(&mut spec).await.unwrap();
                            assert_eq!(spec, b"/data/local/tmp/x.jar,420");
                        }
                        b"DATA" => {
                            let mut data = vec![0u8; length];
                            stream.read_exact(&mut data).await.unwrap();
                            file.extend_from_slice(&data);
                        }
                        b"DONE" => break,
                        other => panic!("{other:?}"),
                    }
                }
                stream.write_all(b"OKAY\0\0\0\0").await.unwrap();
                assert_eq!(received, b"SENDDATADATADONE");
                if let Some(seen) = seen_tx {
                    seen.send(file).unwrap();
                }
            }
        })
        .await;
        let data: Vec<u8> = (0..SYNC_CHUNK + 10).map(|i| (i % 251) as u8).collect();
        adb.push("abc", &data, "/data/local/tmp/x.jar", 0o644).await.unwrap();
        assert_eq!(seen_rx.await.unwrap(), data);
        assert!(matches!(adb.push("abc", b"x", "relative", 0o644).await, Err(Error::Refused(_))));
        assert!(matches!(adb.push("abc", b"x", "/a,b", 0o644).await, Err(Error::Refused(_))));
    }

    #[tokio::test]
    async fn an_install_streams_the_file_and_explains_a_refusal() {
        let folder = tempfile::tempdir().unwrap();
        let apk = folder.path().join("app.apk");
        std::fs::write(&apk, b"PK-not-really-an-apk").unwrap();
        let adb = fake(|mut stream| async move {
            service(&mut stream).await;
            stream.write_all(b"OKAY").await.unwrap();
            let asked = service(&mut stream).await;
            assert_eq!(asked, "exec:cmd package install -r -t -d -S 20");
            stream.write_all(b"OKAY").await.unwrap();
            let mut body = [0u8; 20];
            stream.read_exact(&mut body).await.unwrap();
            assert_eq!(&body, b"PK-not-really-an-apk");
            stream
                .write_all(b"Failure [INSTALL_FAILED_UPDATE_INCOMPATIBLE: Existing package com.x signatures do not match]\n")
                .await
                .unwrap();
        })
        .await;
        let error = adb.install("abc", &apk, LONG).await.unwrap_err().to_string();
        assert!(error.starts_with("A copy signed with a different key is already installed."), "{error}");
        assert!(error.contains("INSTALL_FAILED_UPDATE_INCOMPATIBLE"));
        let missing = adb.install("abc", &folder.path().join("none.apk"), LONG).await;
        assert!(matches!(missing, Err(Error::Refused(_))));
    }

    #[tokio::test]
    async fn reverse_asks_the_device_to_listen_and_waits_for_both_answers() {
        let adb = fake(|mut stream| async move {
            service(&mut stream).await;
            stream.write_all(b"OKAY").await.unwrap();
            assert_eq!(service(&mut stream).await, "reverse:forward:tcp:8081;tcp:19001");
            stream.write_all(b"OKAYOKAY").await.unwrap();
        })
        .await;
        adb.reverse("abc", 8081, 19001).await.unwrap();
    }

    #[tokio::test]
    async fn tracking_yields_each_new_list() {
        let adb = fake(|mut stream| async move {
            assert_eq!(service(&mut stream).await, "host:track-devices-l");
            stream.write_all(b"OKAY").await.unwrap();
            block(&mut stream, "").await;
            block(&mut stream, "emulator-5554 offline\n").await;
            block(&mut stream, "emulator-5554 device model:Pixel_8\n").await;
        })
        .await;
        let mut tracker = adb.track().await.unwrap();
        assert!(tracker.next().await.unwrap().is_empty());
        assert_eq!(tracker.next().await.unwrap()[0].state, DeviceState::Offline);
        assert_eq!(tracker.next().await.unwrap()[0].model.as_deref(), Some("Pixel_8"));
        assert!(tracker.next().await.is_err());
    }
}
