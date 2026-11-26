//! Android virtual devices, written as files.
//!
//! An AVD is a folder with a `config.ini` and a small `.ini` beside it that points at the folder.
//! `avdmanager` writes them, and needs Java to do it; Ghost writes the same files itself. Ghost's AVDs
//! live in Ghost's own folder (`ANDROID_AVD_HOME` when its emulator runs), so the AVDs a person made
//! in Android Studio are never listed, started or changed here.

use std::path::{Path, PathBuf};

use crate::Error;
use crate::repo::ImagePath;

/// The phone an AVD imitates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shape {
    pub width: u32,
    pub height: u32,
    /// Pixels per inch, which sets how large the interface draws.
    pub density: u32,
}

impl Shape {
    /// A current flagship phone's screen: 1080 by 2400 at 420 dpi.
    pub const PHONE: Shape = Shape {
        width: 1080,
        height: 2400,
        density: 420,
    };
}

/// What to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    /// The AVD's name, also its file name: letters, digits, `_`, `-` and `.`.
    pub name: String,
    /// What the person sees it called.
    pub title: String,
    pub image: ImagePath,
    pub shape: Shape,
    /// Memory for the device, in MiB.
    pub ram_mb: u32,
    pub cores: u32,
    /// Its storage, in GiB. The file grows as it is used; this is its limit.
    pub storage_gb: u32,
}

/// An AVD's name may sit in a file name and on a command line.
pub fn checked_name(name: &str) -> Result<&str, Error> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
        && !name.starts_with(['.', '-']);
    if valid {
        Ok(name)
    } else {
        Err(Error::Refused("That is not a device name.".into()))
    }
}

/// The folder an AVD's files are in.
pub fn folder(home: &Path, name: &str) -> PathBuf {
    home.join(format!("{name}.avd"))
}

/// Whether `home` holds an AVD called `name`.
pub fn exists(home: &Path, name: &str) -> bool {
    home.join(format!("{name}.ini")).is_file() && folder(home, name).join("config.ini").is_file()
}

/// The image an existing AVD runs, read back from its configuration.
pub fn image_of(home: &Path, name: &str) -> Option<ImagePath> {
    let config = std::fs::read_to_string(folder(home, name).join("config.ini")).ok()?;
    let line = config
        .lines()
        .find_map(|line| line.strip_prefix("image.sysdir.1="))?;
    let mut parts = line.trim().trim_end_matches(['/', '\\']).split(['/', '\\']);
    if parts.next()? != "system-images" {
        return None;
    }
    let path = format!(
        "system-images;{};{};{}",
        parts.next()?,
        parts.next()?,
        parts.next()?
    );
    parts
        .next()
        .is_none()
        .then(|| ImagePath::parse(&path))
        .flatten()
}

/// Writes the AVD into `home`. An AVD of that name is replaced: its settings, not its storage.
pub fn write(home: &Path, spec: &Spec) -> Result<(), Error> {
    let name = checked_name(&spec.name)?;
    let dir = folder(home, name);
    std::fs::create_dir_all(&dir)?;
    let separator = std::path::MAIN_SEPARATOR;
    let image = format!("{}/", spec.image.folder()).replace('/', &separator.to_string());
    let (arch, abi) = match spec.image.abi.as_str() {
        "arm64-v8a" => ("arm64", "arm64-v8a"),
        _ => ("x86_64", "x86_64"),
    };
    let title: String = spec
        .title
        .chars()
        .filter(|ch| !ch.is_control() && *ch != '=')
        .take(60)
        .collect();
    let config = [
        ("AvdId", name.to_owned()),
        ("PlayStore.enabled", "false".into()),
        ("abi.type", abi.into()),
        ("avd.ini.displayname", title),
        ("avd.ini.encoding", "UTF-8".into()),
        (
            "disk.dataPartition.size",
            format!("{}G", spec.storage_gb.clamp(2, 64)),
        ),
        // Start from the saved state when there is one: seconds instead of a minute.
        ("fastboot.forceColdBoot", "no".into()),
        ("fastboot.forceFastBoot", "yes".into()),
        ("hw.accelerometer", "yes".into()),
        ("hw.audioInput", "no".into()),
        ("hw.battery", "yes".into()),
        ("hw.camera.back", "emulated".into()),
        ("hw.camera.front", "emulated".into()),
        ("hw.cpu.arch", arch.into()),
        ("hw.cpu.ncore", spec.cores.clamp(1, 8).to_string()),
        ("hw.dPad", "no".into()),
        ("hw.gps", "yes".into()),
        ("hw.gpu.enabled", "yes".into()),
        ("hw.gpu.mode", "auto".into()),
        ("hw.keyboard", "yes".into()),
        ("hw.lcd.density", spec.shape.density.to_string()),
        ("hw.lcd.height", spec.shape.height.to_string()),
        ("hw.lcd.width", spec.shape.width.to_string()),
        ("hw.mainKeys", "no".into()),
        ("hw.ramSize", spec.ram_mb.clamp(1024, 8192).to_string()),
        ("hw.sdCard", "no".into()),
        ("hw.sensors.orientation", "yes".into()),
        ("hw.sensors.proximity", "yes".into()),
        ("hw.trackBall", "no".into()),
        ("image.sysdir.1", image),
        ("runtime.network.latency", "none".into()),
        ("runtime.network.speed", "full".into()),
        ("showDeviceFrame", "no".into()),
        ("skin.dynamic", "yes".into()),
        (
            "skin.name",
            format!("{}x{}", spec.shape.width, spec.shape.height),
        ),
        ("skin.path", "_no_skin".into()),
        ("tag.display", "Google APIs".into()),
        ("tag.id", spec.image.tag.clone()),
        ("vm.heapSize", "256".into()),
    ];
    let mut text = String::new();
    for (key, value) in config {
        text.push_str(&format!("{key}={value}\n"));
    }
    std::fs::write(dir.join("config.ini"), text)?;
    let pointer = format!(
        "avd.ini.encoding=UTF-8\npath={}\ntarget={}\n",
        dir.display(),
        spec.image.platform
    );
    std::fs::write(home.join(format!("{name}.ini")), pointer)?;
    Ok(())
}

/// Removes an AVD and everything on it.
pub fn remove(home: &Path, name: &str) -> Result<(), Error> {
    let name = checked_name(name)?;
    let _ = std::fs::remove_file(home.join(format!("{name}.ini")));
    match std::fs::remove_dir_all(folder(home, name)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::Io(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str) -> Spec {
        Spec {
            name: name.into(),
            title: "Ghost phone 1".into(),
            image: ImagePath::parse("system-images;android-36;google_apis;x86_64").unwrap(),
            shape: Shape::PHONE,
            ram_mb: 2048,
            cores: 4,
            storage_gb: 6,
        }
    }

    #[test]
    fn an_avd_is_two_files_the_emulator_reads() {
        let home = tempfile::tempdir().unwrap();
        assert!(!exists(home.path(), "o7_phone_1"));
        write(home.path(), &spec("o7_phone_1")).unwrap();
        assert!(exists(home.path(), "o7_phone_1"));
        let pointer = std::fs::read_to_string(home.path().join("o7_phone_1.ini")).unwrap();
        assert!(pointer.contains("target=android-36\n"));
        assert!(pointer.contains(&format!(
            "path={}",
            folder(home.path(), "o7_phone_1").display()
        )));
        let config =
            std::fs::read_to_string(folder(home.path(), "o7_phone_1").join("config.ini")).unwrap();
        for line in [
            "AvdId=o7_phone_1",
            "abi.type=x86_64",
            "hw.cpu.arch=x86_64",
            "hw.lcd.width=1080",
            "hw.lcd.height=2400",
            "hw.lcd.density=420",
            "hw.ramSize=2048",
            "disk.dataPartition.size=6G",
            "tag.id=google_apis",
            "avd.ini.displayname=Ghost phone 1",
        ] {
            assert!(config.lines().any(|have| have == line), "{line}\n{config}");
        }
        let sysdir = config
            .lines()
            .find(|line| line.starts_with("image.sysdir.1="))
            .unwrap();
        assert!(sysdir.ends_with(std::path::MAIN_SEPARATOR));
        assert!(sysdir.contains("android-36"));
        assert_eq!(image_of(home.path(), "o7_phone_1"), Some(spec("x").image));
        assert_eq!(image_of(home.path(), "missing"), None);
    }

    #[test]
    fn an_arm_image_makes_an_arm_device_and_limits_are_kept() {
        let home = tempfile::tempdir().unwrap();
        let mut arm = spec("arm");
        arm.image = ImagePath::parse("system-images;android-36;google_apis;arm64-v8a").unwrap();
        arm.ram_mb = 64;
        arm.cores = 99;
        arm.storage_gb = 900;
        arm.title = "evil=1\nhw.ramSize=1".into();
        write(home.path(), &arm).unwrap();
        let config =
            std::fs::read_to_string(folder(home.path(), "arm").join("config.ini")).unwrap();
        assert!(config.contains("abi.type=arm64-v8a\n"));
        assert!(config.contains("hw.cpu.arch=arm64\n"));
        assert!(config.contains("hw.ramSize=1024\n"));
        assert!(config.contains("hw.cpu.ncore=8\n"));
        assert!(config.contains("disk.dataPartition.size=64G\n"));
        // A title cannot add a line or a second setting.
        assert_eq!(
            config
                .lines()
                .filter(|line| line.starts_with("hw.ramSize="))
                .count(),
            1
        );
        assert!(config.contains("avd.ini.displayname=evil1hw.ramSize1\n"));
    }

    #[test]
    fn names_stay_inside_the_folder() {
        let home = tempfile::tempdir().unwrap();
        for bad in ["", "..", "../x", "a/b", "a\\b", "-n", ".hidden", "a b", "é"] {
            assert!(checked_name(bad).is_err(), "{bad:?}");
            let mut wrong = spec("ok");
            wrong.name = bad.into();
            assert!(write(home.path(), &wrong).is_err(), "{bad:?}");
            assert!(remove(home.path(), bad).is_err(), "{bad:?}");
        }
        assert!(checked_name(&"a".repeat(65)).is_err());
        assert!(std::fs::read_dir(home.path()).unwrap().next().is_none());
    }

    #[test]
    fn removing_takes_both_files_and_is_calm_about_nothing() {
        let home = tempfile::tempdir().unwrap();
        write(home.path(), &spec("gone")).unwrap();
        std::fs::write(
            folder(home.path(), "gone").join("userdata-qemu.img"),
            b"data",
        )
        .unwrap();
        remove(home.path(), "gone").unwrap();
        assert!(!exists(home.path(), "gone"));
        assert!(!folder(home.path(), "gone").exists());
        remove(home.path(), "gone").unwrap();
    }
}
