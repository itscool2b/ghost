//! What Ghost does on an Android device, each as one or two commands in the device's shell.
//!
//! Every word that did not come from this file is checked ([`shell::package`],
//! [`shell::component`]) or quoted ([`shell::quote`]) before it is put into a command.

use std::path::Path;
use std::time::Duration;

use crate::adb::Adb;
use crate::outline::Outline;
use crate::{Error, shell};

const QUICK: Duration = Duration::from_secs(10);
const SLOW: Duration = Duration::from_secs(40);
/// The largest screenshot accepted (a 4K screen as PNG is well under this).
const PICTURE_LIMIT: usize = 64 * 1024 * 1024;
const TEXT_LIMIT: usize = 4 * 1024 * 1024;

/// One device, by serial.
#[derive(Debug, Clone)]
pub struct Device {
    adb: Adb,
    serial: String,
}

/// Light or dark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Light,
    Dark,
}

/// Which way up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Portrait,
    Landscape,
}

impl Device {
    pub fn new(adb: Adb, serial: impl Into<String>) -> Device {
        Device {
            adb,
            serial: serial.into(),
        }
    }

    pub fn serial(&self) -> &str {
        &self.serial
    }

    async fn run(&self, command: &str, doing: &str) -> Result<String, Error> {
        self.adb
            .shell(&self.serial, command, TEXT_LIMIT, SLOW)
            .await?
            .ok(doing)
    }

    async fn property(&self, name: &str) -> Option<String> {
        let output = self
            .adb
            .shell(&self.serial, &format!("getprop {name}"), 4096, QUICK)
            .await
            .ok()?;
        Some(output.text()).filter(|value| !value.is_empty())
    }

    /// The phone's model (`Pixel 8`).
    pub async fn model(&self) -> Option<String> {
        self.property("ro.product.model").await
    }

    /// The Android version (`16`).
    pub async fn android_version(&self) -> Option<String> {
        self.property("ro.build.version.release").await
    }

    /// The AVD an emulator is running, which tells Ghost's own emulators from anyone else's.
    pub async fn avd_name(&self) -> Option<String> {
        match self.property("ro.boot.qemu.avd_name").await {
            Some(name) => Some(name),
            None => self.property("ro.kernel.qemu.avd_name").await,
        }
    }

    /// The display's size in its natural position, in pixels.
    pub async fn display_size(&self) -> Result<(u32, u32), Error> {
        let said = self.run("wm size", "Reading the screen's size").await?;
        parse_display_size(&said)
            .ok_or_else(|| Error::Device("The device did not say how large its screen is.".into()))
    }

    /// The screen as a PNG, at the device's own size.
    pub async fn screenshot(&self) -> Result<Vec<u8>, Error> {
        let png = self
            .adb
            .exec(&self.serial, "screencap -p", PICTURE_LIMIT, SLOW)
            .await?;
        if png.starts_with(&[0x89, b'P', b'N', b'G']) {
            Ok(png)
        } else {
            Err(Error::Device(
                "The device did not give a picture of its screen (it may be off or locked).".into(),
            ))
        }
    }

    /// The screen as an outline. A screen that is moving cannot be read; it is tried twice.
    pub async fn outline(&self) -> Result<Outline, Error> {
        let mut last = None;
        for attempt in 0..2 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(600)).await;
            }
            let dump = self
                .adb
                .exec(&self.serial, "uiautomator dump /dev/tty", TEXT_LIMIT, SLOW)
                .await?;
            match Outline::parse(&dump) {
                Ok(outline) => return Ok(outline),
                Err(error) => last = Some(error),
            }
        }
        Err(last.unwrap_or(Error::Timeout))
    }

    /// Installs an APK from this computer.
    pub async fn install(&self, apk: &Path) -> Result<(), Error> {
        let is_apk = apk
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("apk"));
        if !is_apk {
            return Err(Error::Refused("Only an .apk file can be installed.".into()));
        }
        self.adb
            .install(&self.serial, apk, Duration::from_secs(300))
            .await
    }

    /// Whether `package` is installed.
    pub async fn installed(&self, package: &str) -> Result<bool, Error> {
        let package = shell::package(package)?;
        let output = self
            .adb
            .shell(&self.serial, &format!("pm path {package}"), 8192, QUICK)
            .await?;
        Ok(output.exit == 0 && output.text().starts_with("package:"))
    }

    /// Starts an app: its launcher screen, or `activity` when one is named. Returns the
    /// component that was started.
    pub async fn launch(&self, package: &str, activity: Option<&str>) -> Result<String, Error> {
        let package = shell::package(package)?;
        if !self.installed(package).await? {
            return Err(Error::Device(format!(
                "{package} is not installed on the device."
            )));
        }
        let component = match activity {
            Some(activity) => format!("{package}/{}", shell::component(activity)?),
            None => {
                let said = self
                    .run(
                        &format!(
                            "cmd package resolve-activity --brief -c android.intent.category.LAUNCHER {package}"
                        ),
                        "Finding the app's first screen",
                    )
                    .await?;
                parse_component(&said, package).ok_or_else(|| {
                    Error::Device(format!(
                        "{package} has no screen to open from the launcher. Name the activity to start."
                    ))
                })?
            }
        };
        let said = self
            .run(&format!("am start -W -n {component}"), "Starting the app")
            .await?;
        match start_error(&said) {
            Some(why) => Err(Error::Device(format!("The app did not start: {why}"))),
            None => Ok(component),
        }
    }

    /// Ends an app's processes.
    pub async fn stop_app(&self, package: &str) -> Result<(), Error> {
        let package = shell::package(package)?;
        self.run(&format!("am force-stop {package}"), "Stopping the app")
            .await
            .map(drop)
    }

    /// Removes an app and its data.
    pub async fn uninstall(&self, package: &str) -> Result<(), Error> {
        let package = shell::package(package)?;
        let said = self
            .run(&format!("pm uninstall {package}"), "Removing the app")
            .await?;
        if said.contains("Success") {
            Ok(())
        } else {
            Err(Error::Device(format!(
                "The app was not removed: {}",
                shell::shown(&said)
            )))
        }
    }

    /// Wipes an app's data, as if it had just been installed.
    pub async fn clear_data(&self, package: &str) -> Result<(), Error> {
        let package = shell::package(package)?;
        self.run(&format!("pm clear {package}"), "Clearing the app's data")
            .await
            .map(drop)
    }

    /// Opens a link as a tap on it would: a web page, or an app's own deep link.
    pub async fn open_link(&self, link: &str) -> Result<(), Error> {
        let link = link.trim();
        let has_scheme = link.split_once(':').is_some_and(|(scheme, rest)| {
            !rest.is_empty()
                && scheme
                    .chars()
                    .next()
                    .is_some_and(|first| first.is_ascii_alphabetic())
                && scheme
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))
        });
        if !has_scheme || link.len() > 2000 || link.chars().any(char::is_control) {
            return Err(Error::Refused(
                "A link needs a scheme, like https://example.com or myapp://screen.".into(),
            ));
        }
        let said = self
            .run(
                &format!(
                    "am start -W -a android.intent.action.VIEW -d {}",
                    shell::quote(link)?
                ),
                "Opening the link",
            )
            .await?;
        match start_error(&said) {
            Some(why) => Err(Error::Device(format!(
                "Nothing on the device opened the link: {why}"
            ))),
            None => Ok(()),
        }
    }

    /// The package of the app in front.
    pub async fn foreground(&self) -> Option<String> {
        let output = self
            .adb
            .shell(
                &self.serial,
                "dumpsys window | grep -m1 mCurrentFocus",
                8192,
                QUICK,
            )
            .await
            .ok()?;
        parse_focus(&output.text())
    }

    /// The last `lines` of the log: `package`'s own lines while it runs, and its last crash
    /// whether it runs or not.
    pub async fn logs(&self, package: Option<&str>, lines: u32) -> Result<String, Error> {
        let lines = lines.clamp(1, 500);
        let mut out = String::new();
        match package {
            Some(package) => {
                let package = shell::package(package)?;
                let pid = self
                    .adb
                    .shell(&self.serial, &format!("pidof {package}"), 256, QUICK)
                    .await?
                    .text();
                let pid = pid
                    .split_whitespace()
                    .next()
                    .and_then(|pid| pid.parse::<u32>().ok());
                match pid {
                    Some(pid) => {
                        let log = self
                            .run(
                                &format!("logcat -d -v time -t {lines} --pid={pid}"),
                                "Reading the log",
                            )
                            .await?;
                        out.push_str(&format!("Log of {package} (last {lines} lines):\n{log}\n"));
                    }
                    None => out.push_str(&format!("{package} is not running.\n")),
                }
                let crash = self
                    .run("logcat -d -b crash -v time -t 80", "Reading the log")
                    .await?;
                if crash.contains(package) {
                    out.push_str(&format!("\nIts last crash:\n{crash}\n"));
                }
            }
            None => {
                let log = self
                    .run(&format!("logcat -d -v time -t {lines}"), "Reading the log")
                    .await?;
                out.push_str(&format!("The device's log (last {lines} lines):\n{log}\n"));
            }
        }
        Ok(tail(&out, 60_000))
    }

    /// Switches the device between light and dark.
    pub async fn set_appearance(&self, appearance: Appearance) -> Result<(), Error> {
        let night = match appearance {
            Appearance::Dark => "yes",
            Appearance::Light => "no",
        };
        self.run(
            &format!("cmd uimode night {night}"),
            "Changing the appearance",
        )
        .await
        .map(drop)
    }

    /// Sets how large text draws: 1.0 is the default, 2.0 the largest Android offers.
    pub async fn set_text_scale(&self, scale: f32) -> Result<(), Error> {
        if !(0.85..=2.0).contains(&scale) {
            return Err(Error::Refused(
                "Text size is a number from 0.85 to 2.".into(),
            ));
        }
        self.run(
            &format!("settings put system font_scale {scale:.2}"),
            "Changing the text size",
        )
        .await
        .map(drop)
    }

    /// Turns the device, and stops it turning by itself.
    pub async fn set_orientation(&self, orientation: Orientation) -> Result<(), Error> {
        let turns = match orientation {
            Orientation::Portrait => 0,
            Orientation::Landscape => 1,
        };
        self.run(
            &format!(
                "settings put system accelerometer_rotation 0 && settings put system user_rotation {turns}"
            ),
            "Turning the device",
        )
        .await
        .map(drop)
    }

    /// Turns the device through the window manager's own command (`wm user-rotation`), for a
    /// system that did not follow the setting [`Device::set_orientation`] writes.
    pub async fn lock_rotation(&self, orientation: Orientation) -> Result<(), Error> {
        let turns = match orientation {
            Orientation::Portrait => 0,
            Orientation::Landscape => 1,
        };
        self.run(
            &format!("wm user-rotation lock {turns}"),
            "Turning the device",
        )
        .await
        .map(drop)
    }

    /// Whether the main display is wider than tall now, as the window manager has it. `None`
    /// when that cannot be read.
    pub async fn on_its_side(&self) -> Option<bool> {
        let text = self
            .adb
            .shell(&self.serial, "dumpsys window displays", TEXT_LIMIT, QUICK)
            .await
            .ok()?
            .ok("Reading the display")
            .ok()?;
        main_display_on_its_side(&text)
    }

    /// Gives or takes away one of an app's permissions (`android.permission.CAMERA`).
    pub async fn set_permission(
        &self,
        package: &str,
        permission: &str,
        granted: bool,
    ) -> Result<(), Error> {
        let package = shell::package(package)?;
        let permission = shell::component(permission)?;
        let verb = if granted { "grant" } else { "revoke" };
        self.run(
            &format!("pm {verb} {package} {permission}"),
            "Changing the permission",
        )
        .await
        .map(drop)
    }
}

/// `Physical size: 1080x2400`, and `Override size:` when a person changed it.
fn parse_display_size(said: &str) -> Option<(u32, u32)> {
    let size = |prefix: &str| {
        said.lines().find_map(|line| {
            let (width, height) = line.trim().strip_prefix(prefix)?.trim().split_once('x')?;
            Some((width.parse().ok()?, height.parse().ok()?))
        })
    };
    size("Override size:")
        .or_else(|| size("Physical size:"))
        .filter(|(w, h)| *w > 0 && *h > 0)
}

/// From `dumpsys window displays`: whether display 0 is wider than tall now. Its block starts
/// `Display: mDisplayId=0` and soon has `cur=2400x1080`; other displays (scrcpy's own) are not
/// read.
fn main_display_on_its_side(said: &str) -> Option<bool> {
    let mut lines = said.lines().skip_while(|line| {
        line.trim_start()
            .strip_prefix("Display: mDisplayId=0")
            .is_none_or(|rest| !(rest.is_empty() || rest.starts_with(' ')))
    });
    lines.next()?;
    lines.take(4).find_map(|line| {
        let (_, after) = line.split_once(" cur=")?;
        let (width, height) = after.split_whitespace().next()?.split_once('x')?;
        let (width, height): (u32, u32) = (width.parse().ok()?, height.parse().ok()?);
        Some(width > height)
    })
}

/// The last line that names a component of `package` (`com.example/.MainActivity`).
fn parse_component(said: &str, package: &str) -> Option<String> {
    said.lines().rev().map(str::trim).find_map(|line| {
        let (owner, activity) = line.split_once('/')?;
        (owner == package && shell::component(activity).is_ok()).then(|| line.to_owned())
    })
}

/// `am start`'s complaint, when it made one.
fn start_error(said: &str) -> Option<String> {
    let lines = || said.lines().map(str::trim);
    lines()
        .find_map(|line| line.strip_prefix("Error:"))
        .or_else(|| lines().find(|line| line.starts_with("Error") || line.starts_with("Exception")))
        .map(|line| shell::shown(line.trim()))
}

/// `mCurrentFocus=Window{4f5 u0 com.example.app/com.example.app.MainActivity}`.
fn parse_focus(line: &str) -> Option<String> {
    let inside = line.split_once('{')?.1.trim_end_matches('}');
    let component = inside.split_whitespace().last()?;
    let package = component.split('/').next()?;
    shell::package(package).ok().map(str::to_owned)
}

/// The end of `text`, cut at a line, when it is longer than `limit` bytes.
fn tail(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut start = text.len() - limit;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let cut = &text[start..];
    let cut = cut.split_once('\n').map_or(cut, |(_, rest)| rest);
    format!("(earlier lines left out)\n{cut}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_screens_size_prefers_an_override() {
        assert_eq!(
            parse_display_size("Physical size: 1080x2400\n"),
            Some((1080, 2400))
        );
        assert_eq!(
            parse_display_size("Physical size: 1080x2400\nOverride size: 720x1600\n"),
            Some((720, 1600))
        );
        assert_eq!(parse_display_size("nothing"), None);
        assert_eq!(parse_display_size("Physical size: 0x0"), None);
        assert_eq!(parse_display_size("Physical size: axb"), None);
    }

    #[test]
    fn the_main_displays_turn_is_read_from_the_window_manager() {
        // As Android 17's emulator printed it, with scrcpy's display first (24 November 2026).
        let said = "WINDOW MANAGER DISPLAY CONTENTS (dumpsys window displays)\n  Display: mDisplayId=7\n    init=1600x720 1dpi mMinSizeOfResizeableTaskDp=220 cur=1600x720 app=1600x720\n    mRotation=0\n  Display: mDisplayId=0 (organized)\n    init=1080x2400 420dpi mMinSizeOfResizeableTaskDp=220 cur=2400x1080 app=2400x1080 rng=1080x1080-2400x2400\n    deferred=false mLayoutNeeded=false\n";
        assert_eq!(main_display_on_its_side(said), Some(true));
        let upright = said.replace("cur=2400x1080", "cur=1080x2400");
        assert_eq!(main_display_on_its_side(&upright), Some(false));
        // Display 10 is not display 0, and a block without its size says nothing.
        assert_eq!(
            main_display_on_its_side("  Display: mDisplayId=10\n    cur=2400x1080\n"),
            None
        );
        assert_eq!(
            main_display_on_its_side("  Display: mDisplayId=0\n    mRotation=0\n"),
            None
        );
        assert_eq!(main_display_on_its_side(""), None);
    }

    #[test]
    fn the_launcher_screen_is_the_apps_own_component() {
        let said = "priority=0 preferredOrder=0 match=0x108000 specificIndex=-1 isDefault=false\ncom.example.app/.MainActivity\n";
        assert_eq!(
            parse_component(said, "com.example.app").as_deref(),
            Some("com.example.app/.MainActivity")
        );
        assert_eq!(
            parse_component("No activity found\n", "com.example.app"),
            None
        );
        // Another app's component, or one with shell characters, is not taken.
        assert_eq!(
            parse_component("com.other/.Main\n", "com.example.app"),
            None
        );
        assert_eq!(
            parse_component("com.example.app/.Main;reboot\n", "com.example.app"),
            None
        );
    }

    #[test]
    fn am_starts_complaints_are_found() {
        assert_eq!(
            start_error("Starting: Intent { ... }\nStatus: ok\nLaunchState: COLD\n"),
            None
        );
        assert_eq!(
            start_error(
                "Starting: Intent\nError type 3\nError: Activity class {a/b} does not exist.\n"
            )
            .as_deref(),
            Some("Activity class {a/b} does not exist.")
        );
        assert_eq!(
            start_error("Error: Activity not started, unable to resolve Intent { act=VIEW }")
                .as_deref(),
            Some("Activity not started, unable to resolve Intent { act=VIEW }")
        );
    }

    #[test]
    fn the_app_in_front_is_read_from_the_focused_window() {
        assert_eq!(
            parse_focus(
                "  mCurrentFocus=Window{4f5a1b2 u0 com.example.app/com.example.app.MainActivity}"
            )
            .as_deref(),
            Some("com.example.app")
        );
        assert_eq!(
            parse_focus("mCurrentFocus=Window{1 u0 NotificationShade}"),
            None
        );
        assert_eq!(parse_focus("mCurrentFocus=null"), None);
        assert_eq!(parse_focus(""), None);
    }

    #[test]
    fn a_long_log_keeps_its_end_from_a_whole_line() {
        assert_eq!(tail("short", 100), "short");
        let text = "line one\nline two\nline three\n";
        assert_eq!(tail(text, 14), "(earlier lines left out)\nline three\n");
        let wide = "é".repeat(50);
        assert!(tail(&wide, 11).ends_with('é'));
    }

    #[tokio::test]
    async fn words_from_outside_never_reach_a_command() {
        // Nothing answers on this port: a refusal must come before any connection is tried.
        let device = Device::new(Adb::at(1), "emulator-5580");
        for package in ["", "app", "com.app; reboot", "-r", "com.app --user 0"] {
            assert!(
                matches!(device.launch(package, None).await, Err(Error::Refused(_))),
                "{package:?}"
            );
            assert!(matches!(
                device.stop_app(package).await,
                Err(Error::Refused(_))
            ));
            assert!(matches!(
                device.uninstall(package).await,
                Err(Error::Refused(_))
            ));
            assert!(matches!(
                device.clear_data(package).await,
                Err(Error::Refused(_))
            ));
            assert!(matches!(
                device.logs(Some(package), 10).await,
                Err(Error::Refused(_))
            ));
            assert!(matches!(
                device
                    .set_permission(package, "android.permission.CAMERA", true)
                    .await,
                Err(Error::Refused(_))
            ));
        }
        assert!(matches!(
            device
                .set_permission("com.example.app", "CAMERA; id", true)
                .await,
            Err(Error::Refused(_))
        ));
        for link in [
            "",
            "example.com",
            "://x",
            "https:",
            "1x://a",
            "https://a\nb",
            &format!("https://{}", "a".repeat(3000)),
        ] {
            assert!(
                matches!(device.open_link(link).await, Err(Error::Refused(_))),
                "{link:?}"
            );
        }
        assert!(matches!(
            device.set_text_scale(9.0).await,
            Err(Error::Refused(_))
        ));
        assert!(matches!(
            device.install(Path::new("/tmp/app.zip")).await,
            Err(Error::Refused(_))
        ));
    }
}
