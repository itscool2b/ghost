//! The Android emulator's life: started with no window of its own, watched until it has booted,
//! and stopped so that it saves its state for a quick start next time.

use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};

use crate::Error;
use crate::adb::Adb;

/// The console ports o7's emulators take: even numbers, each with the odd one after it for adb.
/// Android Studio counts up from 5554; starting here keeps the two apart.
const FIRST_PORT: u16 = 5580;
const LAST_PORT: u16 = 5598;

/// How to start one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    /// The SDK that holds the emulator and the AVD's image.
    pub sdk_root: PathBuf,
    /// o7's folder of AVDs.
    pub avd_home: PathBuf,
    /// Where the emulator keeps its own settings, apart from the person's `~/.android`.
    pub emulator_home: PathBuf,
    pub name: String,
    pub port: u16,
    /// A file for what the emulator prints.
    pub log: PathBuf,
}

/// A running emulator. Dropping it ends the process and everything it started (the virtual
/// machine is a child of the emulator's own launcher on Windows).
#[derive(Debug)]
pub struct Emulator {
    child: Box<dyn ChildWrapper>,
    pub port: u16,
    pub serial: String,
    log: PathBuf,
}

/// The serial adb gives the emulator on console port `port`.
pub fn serial(port: u16) -> String {
    format!("emulator-{port}")
}

/// A console port nobody is using, not among `taken`.
pub fn free_port(taken: &[u16]) -> Option<u16> {
    (FIRST_PORT..=LAST_PORT).step_by(2).find(|port| {
        !taken.contains(port)
            && [*port, *port + 1]
                .iter()
                .all(|port| TcpListener::bind((Ipv4Addr::LOCALHOST, *port)).is_ok())
    })
}

/// Starts the emulator. It has booted once [`Emulator::wait_booted`] returns.
pub fn start(launch: &Launch) -> Result<Emulator, Error> {
    let name = crate::avd::checked_name(&launch.name)?;
    if let Some(parent) = launch.log.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir_all(&launch.emulator_home)?;
    let log = std::fs::File::create(&launch.log)?;
    let mut command = crate::command(&crate::sdk::emulator_in(&launch.sdk_root));
    command
        .arg("-avd")
        .arg(name)
        .arg("-no-window")
        .arg("-no-audio")
        .arg("-no-boot-anim")
        .arg("-no-metrics")
        .arg("-port")
        .arg(launch.port.to_string())
        .env("ANDROID_SDK_ROOT", &launch.sdk_root)
        .env("ANDROID_HOME", &launch.sdk_root)
        .env("ANDROID_AVD_HOME", &launch.avd_home)
        .env("ANDROID_EMULATOR_HOME", &launch.emulator_home)
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    let mut wrapped = CommandWrap::from(command);
    wrapped.wrap(KillOnDrop);
    #[cfg(unix)]
    wrapped.wrap(process_wrap::tokio::ProcessGroup::leader());
    #[cfg(windows)]
    {
        use process_wrap::tokio::{CreationFlags, JobObject};
        wrapped.wrap(CreationFlags(
            windows::Win32::System::Threading::CREATE_NO_WINDOW,
        ));
        wrapped.wrap(JobObject);
    }
    let child = wrapped
        .spawn()
        .map_err(|error| Error::Device(format!("The emulator could not be started: {error}")))?;
    Ok(Emulator {
        child,
        port: launch.port,
        serial: serial(launch.port),
        log: launch.log.clone(),
    })
}

impl Emulator {
    /// The process id, for the Processes page.
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// Whether the process has ended.
    pub fn ended(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }

    /// Why the emulator ended, from what it printed.
    pub fn failure(&self) -> String {
        let log = std::fs::read(&self.log).unwrap_or_default();
        explain(&String::from_utf8_lossy(&log))
    }

    /// Stops the emulator: asked first, so it saves its state, then killed if it does not go.
    pub async fn stop(mut self, adb_program: &Path, adb: &Adb, patience: Duration) {
        let mut ask = crate::command(adb_program);
        ask.arg("-P")
            .arg(adb.port().to_string())
            .arg("-s")
            .arg(&self.serial)
            .arg("emu")
            .arg("kill");
        ask.stdout(Stdio::null()).stderr(Stdio::null());
        let _ = tokio::time::timeout(Duration::from_secs(10), ask.status()).await;
        if tokio::time::timeout(patience, self.child.wait())
            .await
            .is_ok()
        {
            return;
        }
        self.kill().await;
    }

    /// Ends the process and what it started, at once.
    pub async fn kill(mut self) {
        let _ = Box::into_pin(self.child.kill()).await;
    }
}

/// Whether Android on `serial` has finished starting.
pub async fn booted(adb: &Adb, serial: &str) -> bool {
    let asked = adb
        .shell(
            serial,
            "getprop sys.boot_completed",
            64,
            Duration::from_secs(5),
        )
        .await;
    matches!(&asked, Ok(output) if output.text() == "1")
}

/// The emulator's last words, turned into what the person can do about it.
fn explain(log: &str) -> String {
    let lower = log.to_ascii_lowercase();
    let known = if lower.contains("not enough disk space") || lower.contains("not enough space") {
        Some("The disk is too full for the emulator.")
    } else if lower
        .lines()
        .any(|line| line.contains("whpx") && (line.contains(" not ") || line.contains("fail")))
    {
        Some(
            "Windows Hypervisor Platform is not turned on. Turn it on in \"Turn Windows features on or off\", then restart the computer.",
        )
    } else if lower.contains("/dev/kvm") {
        Some("This user cannot use KVM. Add the user to the kvm group, then sign in again.")
    } else if lower.contains("running multiple emulators with the same avd") {
        Some("That device is already running.")
    } else if lower.contains("cannot find avd system path")
        || lower.contains("broken avd system path")
    {
        Some("The device's Android image is missing. Set the device up again.")
    } else {
        None
    };
    if let Some(known) = known {
        return known.to_owned();
    }
    let last = log
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| {
            let line = line.to_ascii_lowercase();
            line.contains("error") || line.contains("fatal") || line.contains("panic")
        })
        .or_else(|| {
            log.lines()
                .rev()
                .map(str::trim)
                .find(|line| !line.is_empty())
        });
    match last {
        Some(line) => {
            let line: String = line.chars().take(300).collect();
            format!("The emulator stopped while starting: {line}")
        }
        None => "The emulator stopped while starting and gave no reason.".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_are_even_free_and_not_taken() {
        let first = free_port(&[]).unwrap();
        assert!((FIRST_PORT..=LAST_PORT).contains(&first) && first.is_multiple_of(2));
        let held = TcpListener::bind((Ipv4Addr::LOCALHOST, first));
        let second = free_port(&[]).unwrap();
        assert_ne!(first, second, "a port in use is skipped");
        drop(held);
        let third = free_port(&[first]).unwrap();
        assert_ne!(third, first, "a port that is taken is skipped");
        let all: Vec<u16> = (FIRST_PORT..=LAST_PORT).step_by(2).collect();
        assert_eq!(free_port(&all), None);
        assert_eq!(serial(5580), "emulator-5580");
    }

    #[test]
    fn known_failures_say_what_to_do() {
        assert!(
            explain("ERROR | Not enough disk space to run AVD 'x'. Exiting...")
                .contains("disk is too full")
        );
        assert!(
            explain("WHPX is not installed or not usable").contains("Windows Hypervisor Platform")
        );
        assert!(explain("ERROR | /dev/kvm device: permission denied").contains("kvm group"));
        assert!(
            explain(
                "ERROR | Running multiple emulators with the same AVD is an experimental feature."
            )
            .contains("already running")
        );
        assert!(
            explain("PANIC: Cannot find AVD system path. Please define ANDROID_SDK_ROOT")
                .contains("image is missing")
        );
        assert_eq!(
            explain("INFO | boot\nFATAL | something odd happened\nINFO | bye\n"),
            "The emulator stopped while starting: FATAL | something odd happened"
        );
        assert_eq!(
            explain("\n\n"),
            "The emulator stopped while starting and gave no reason."
        );
        assert_eq!(
            explain("just a line\n"),
            "The emulator stopped while starting: just a line"
        );
    }

    #[test]
    fn a_bad_name_never_reaches_the_command_line() {
        let folder = tempfile::tempdir().unwrap();
        let launch = Launch {
            sdk_root: folder.path().to_owned(),
            avd_home: folder.path().to_owned(),
            emulator_home: folder.path().join("home"),
            name: "-wipe-data".into(),
            port: 5580,
            log: folder.path().join("log.txt"),
        };
        assert!(matches!(start(&launch), Err(Error::Refused(_))));
        assert!(!folder.path().join("log.txt").exists());
    }
}
