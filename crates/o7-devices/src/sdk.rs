//! Which of Android's tools are on this computer, and where.
//!
//! o7 keeps its own SDK folder and also looks where a person's own SDK usually is
//! (`ANDROID_HOME`, `ANDROID_SDK_ROOT`, Android Studio's default folder). It reads those; it
//! writes only its own.

use std::path::{Path, PathBuf};

use crate::repo::{Host, ImagePath};

/// Where to look, taken from the environment once so tests can say their own.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Places {
    pub android_home: Option<PathBuf>,
    pub android_sdk_root: Option<PathBuf>,
    /// Android Studio's default SDK folder for this system and user.
    pub studio_default: Option<PathBuf>,
}

impl Places {
    /// This process's environment.
    pub fn current() -> Places {
        let folder = |key: &str| {
            std::env::var_os(key).filter(|value| !value.is_empty()).map(PathBuf::from)
        };
        let studio_default = match std::env::consts::OS {
            "windows" => folder("LOCALAPPDATA").map(|base| base.join("Android").join("Sdk")),
            "macos" => folder("HOME").map(|home| home.join("Library/Android/sdk")),
            _ => folder("HOME").map(|home| home.join("Android/Sdk")),
        };
        Places {
            android_home: folder("ANDROID_HOME"),
            android_sdk_root: folder("ANDROID_SDK_ROOT"),
            studio_default,
        }
    }
}

/// A system image found in an SDK.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledImage {
    pub path: ImagePath,
    /// The SDK it is in.
    pub root: PathBuf,
}

/// What was found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Installed {
    /// The `adb` program.
    pub adb: Option<PathBuf>,
    /// The SDK the emulator runs from: one that holds both the emulator and an image it can run.
    pub emulator_root: Option<PathBuf>,
    /// The images in [`Installed::emulator_root`] for this computer's processor, newest first.
    pub images: Vec<InstalledImage>,
    /// Whether o7's own folder already holds the emulator, and an image: what a fetch can skip.
    pub own_emulator: bool,
    pub own_image: bool,
}

impl Installed {
    /// Whether an emulator can be started.
    pub fn can_emulate(&self) -> bool {
        self.adb.is_some() && self.emulator_root.is_some() && !self.images.is_empty()
    }
}

fn program(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_owned() }
}

/// `root/platform-tools/adb`.
pub fn adb_in(root: &Path) -> PathBuf {
    root.join("platform-tools").join(program("adb"))
}

/// `root/emulator/emulator`.
pub fn emulator_in(root: &Path) -> PathBuf {
    root.join("emulator").join(program("emulator"))
}

/// The images in `root` that run on `host`, the highest Android first.
pub fn images_in(root: &Path, host: Host) -> Vec<InstalledImage> {
    let mut found = Vec::new();
    let abi = host.image_abi();
    let folders = |path: &Path| -> Vec<(String, PathBuf)> {
        std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| Some((entry.file_name().into_string().ok()?, entry.path())))
            .collect()
    };
    for (platform, platform_dir) in folders(&root.join("system-images")) {
        for (tag, tag_dir) in folders(&platform_dir) {
            let image_dir = tag_dir.join(abi);
            if !image_dir.join("system.img").is_file() {
                continue;
            }
            let path = format!("system-images;{platform};{tag};{abi}");
            if let Some(path) = ImagePath::parse(&path) {
                found.push(InstalledImage { path, root: root.to_owned() });
            }
        }
    }
    // Released versions before previews, the highest first; Google's APIs before a bare image.
    found.sort_by(|a, b| {
        let rank = |image: &InstalledImage| (image.path.api(), image.path.tag == "google_apis");
        rank(b).cmp(&rank(a)).then_with(|| a.path.platform.cmp(&b.path.platform))
    });
    found
}

/// Looks in o7's own SDK folder first, then in the person's.
pub fn find(own: &Path, places: &Places, host: Host) -> Installed {
    let mut roots = vec![own.to_owned()];
    for root in [&places.android_home, &places.android_sdk_root, &places.studio_default]
        .into_iter()
        .flatten()
    {
        if root.is_dir() && !roots.contains(root) {
            roots.push(root.clone());
        }
    }
    let mut installed = Installed {
        adb: roots.iter().map(|root| adb_in(root)).find(|adb| adb.is_file()),
        own_emulator: emulator_in(own).is_file(),
        own_image: !images_in(own, host).is_empty(),
        ..Installed::default()
    };
    for root in &roots {
        if !emulator_in(root).is_file() {
            continue;
        }
        let images = images_in(root, host);
        if !images.is_empty() {
            installed.emulator_root = Some(root.clone());
            installed.images = images;
            break;
        }
    }
    installed
}

/// Whether this computer can run the emulator fast, and the emulator's own words about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Acceleration {
    pub usable: bool,
    /// What `emulator -accel-check` said (`WHPX(10.0.26100) is installed and usable.`).
    pub detail: String,
}

/// Asks the emulator in `root` whether hardware acceleration works.
pub async fn acceleration(root: &Path) -> Acceleration {
    let mut command = crate::command(&emulator_in(root));
    command.arg("-accel-check");
    let ran = tokio::time::timeout(std::time::Duration::from_secs(20), command.output()).await;
    match ran {
        Ok(Ok(output)) => parse_acceleration(&String::from_utf8_lossy(&output.stdout)),
        Ok(Err(error)) => Acceleration {
            usable: false,
            detail: format!("The emulator could not be run: {error}"),
        },
        Err(_) => Acceleration { usable: false, detail: "The emulator did not answer.".into() },
    }
}

/// `accel:`, a status number (0 is good), one line of explanation, `accel`.
fn parse_acceleration(output: &str) -> Acceleration {
    let mut lines = output.lines().map(str::trim).skip_while(|line| *line != "accel:").skip(1);
    let status = lines.next().and_then(|line| line.parse::<i32>().ok());
    let detail = lines.next().filter(|line| *line != "accel").unwrap_or_default().to_owned();
    match status {
        Some(0) => Acceleration { usable: true, detail },
        Some(_) => Acceleration { usable: false, detail },
        None => Acceleration {
            usable: false,
            detail: "The emulator did not say whether acceleration works.".into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::{HostArch, HostOs};

    const HOST: Host = Host { os: HostOs::Linux, arch: HostArch::X64 };

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }

    fn image(root: &Path, platform: &str, tag: &str, abi: &str) {
        touch(&root.join("system-images").join(platform).join(tag).join(abi).join("system.img"));
    }

    #[test]
    fn nothing_installed_is_nothing_found() {
        let own = tempfile::tempdir().unwrap();
        let found = find(own.path(), &Places::default(), HOST);
        assert_eq!(found, Installed::default());
        assert!(!found.can_emulate());
    }

    #[test]
    fn o7s_own_folder_is_used_before_the_persons() {
        let own = tempfile::tempdir().unwrap();
        let theirs = tempfile::tempdir().unwrap();
        for root in [own.path(), theirs.path()] {
            touch(&adb_in(root));
            touch(&emulator_in(root));
            image(root, "android-36", "google_apis", "x86_64");
        }
        let places = Places { android_home: Some(theirs.path().to_owned()), ..Places::default() };
        let found = find(own.path(), &places, HOST);
        assert_eq!(found.adb, Some(adb_in(own.path())));
        assert_eq!(found.emulator_root.as_deref(), Some(own.path()));
        assert!(found.own_emulator && found.own_image && found.can_emulate());
    }

    #[test]
    fn a_persons_complete_sdk_is_found_and_only_read() {
        let own = tempfile::tempdir().unwrap();
        let theirs = tempfile::tempdir().unwrap();
        touch(&adb_in(theirs.path()));
        touch(&emulator_in(theirs.path()));
        image(theirs.path(), "android-35", "google_apis", "x86_64");
        image(theirs.path(), "android-36", "default", "x86_64");
        image(theirs.path(), "android-36", "google_apis", "x86_64");
        image(theirs.path(), "android-36-ext19", "google_apis", "x86_64");
        image(theirs.path(), "android-37", "google_apis", "arm64-v8a");
        let places = Places {
            android_sdk_root: Some(theirs.path().to_owned()),
            studio_default: Some(own.path().join("missing")),
            ..Places::default()
        };
        let found = find(own.path(), &places, HOST);
        assert!(found.can_emulate());
        assert!(!found.own_emulator && !found.own_image);
        assert_eq!(found.emulator_root.as_deref(), Some(theirs.path()));
        let order: Vec<String> = found
            .images
            .iter()
            .map(|image| format!("{}/{}", image.path.platform, image.path.tag))
            .collect();
        // The ARM image is not listed on an x64 computer; the extension image sorts last.
        assert_eq!(
            order,
            ["android-36/google_apis", "android-36/default", "android-35/google_apis", "android-36-ext19/google_apis"]
        );
        assert!(std::fs::read_dir(own.path()).unwrap().next().is_none(), "nothing was written");
    }

    #[test]
    fn an_emulator_without_an_image_cannot_start() {
        let own = tempfile::tempdir().unwrap();
        touch(&adb_in(own.path()));
        touch(&emulator_in(own.path()));
        image(own.path(), "android-36", "google_apis", "arm64-v8a");
        let found = find(own.path(), &Places::default(), HOST);
        assert!(found.own_emulator && !found.own_image);
        assert!(found.emulator_root.is_none() && !found.can_emulate());
    }

    #[test]
    fn the_emulators_verdict_is_read() {
        let good = parse_acceleration("accel:\n0\nWHPX(10.0.26300) is installed and usable.\naccel\n");
        assert!(good.usable);
        assert_eq!(good.detail, "WHPX(10.0.26300) is installed and usable.");
        let bad = parse_acceleration(
            "INFO | noise\naccel:\n6\nPlease ensure Windows Hypervisor Platform (WHPX) is properly installed and usable.\naccel\n",
        );
        assert!(!bad.usable);
        assert!(bad.detail.contains("Windows Hypervisor Platform"));
        assert!(!parse_acceleration("").usable);
        assert!(!parse_acceleration("accel:\nnot a number\n").usable);
    }
}
