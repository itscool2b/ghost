//! Godot (<https://godotengine.org>, MIT), the engine Ghost runs games with itself.
//!
//! A Godot already on this computer is used when it fits the project. Otherwise Ghost fetches one
//! release, pinned here by the size and SHA-512 Godot publishes in its own `SHA512-SUMS.txt`,
//! from Godot's GitHub release, checks it before anything in it is unpacked, and keeps it in its
//! own folder (third-party/godot.md). The person starts that fetch; no agent tool does. Ghost does
//! not ship Godot.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use ghost_fetch::{Digest, DownloadError, Pin, Progress};

use crate::Error;

/// The release Ghost fetches.
pub const VERSION: &str = "4.7.2";
/// The oldest Godot Ghost runs a game with: the first that starts a game parented to another
/// window (`--wid`), which is how the game stays out of sight.
pub const OLDEST: (u8, u8) = (4, 4);
/// The hosts a GitHub release download passes through. No other is followed.
static HOSTS: [&str; 3] = [
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];
/// Godot unpacked: the editor program, and on a Mac its whole bundle.
const UNPACK_LIMIT: u64 = 1024 * 1024 * 1024;

/// One platform's archive of the pinned release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Release {
    pub url: &'static str,
    pub size: u64,
    pub sha512: &'static str,
    /// The program inside the unpacked archive.
    pub program: &'static str,
}

const WINDOWS_X64: Release = Release {
    url: "https://github.com/godotengine/godot/releases/download/4.7.2-stable/Godot_v4.7.2-stable_win64.exe.zip",
    size: 86_013_866,
    sha512: "83decd58fdf67b9d657958a1ae6bf1929c20785315a81effe245874cdc57acb709bf868e00778a96984338c1b29dafdb453c6847747694621c6ecf5da2259993",
    program: "Godot_v4.7.2-stable_win64.exe",
};
const WINDOWS_ARM64: Release = Release {
    url: "https://github.com/godotengine/godot/releases/download/4.7.2-stable/Godot_v4.7.2-stable_windows_arm64.exe.zip",
    size: 84_263_370,
    sha512: "683f8dd9fb087db79dfbbc52d5b2209df98218a4fef0d10d8478ec2230ae8db6032a36929677479b9f9c5a6aa0c51ee359d7e57eb5abbcb6cef4998526dec5a6",
    program: "Godot_v4.7.2-stable_windows_arm64.exe",
};
const LINUX_X64: Release = Release {
    url: "https://github.com/godotengine/godot/releases/download/4.7.2-stable/Godot_v4.7.2-stable_linux.x86_64.zip",
    size: 77_860_424,
    sha512: "9aa00f7a605200940bce3027a567b782f49bd8e940dd06ae9e987bd65aee1b1467edd56ed84fcdcbdd44354bf613bdbb4e5d2913e925850368e150c59ed54c65",
    program: "Godot_v4.7.2-stable_linux.x86_64",
};
const LINUX_ARM64: Release = Release {
    url: "https://github.com/godotengine/godot/releases/download/4.7.2-stable/Godot_v4.7.2-stable_linux.arm64.zip",
    size: 77_008_699,
    sha512: "dd59918da086bd49bde2f5450b5e567ff8650cbde9abbd7b8f4ca1197ff8c609baa38834666d032deafb47099078d7822279e2a0e06e5665745468f26533e7e2",
    program: "Godot_v4.7.2-stable_linux.arm64",
};
const MACOS: Release = Release {
    url: "https://github.com/godotengine/godot/releases/download/4.7.2-stable/Godot_v4.7.2-stable_macos.universal.zip",
    size: 170_622_178,
    sha512: "38aa16e5bba2083941fc5b3e54be0089bd4cc35e32415f5b9fd9a8a6a7b9818255d44532ea8ef94b5aef56c4b407c2d634fa4f657e4ebe681ebbf59b7bac69ca",
    program: "Godot.app/Contents/MacOS/Godot",
};

/// Every platform's archive, for the tests that hold the pins to their form.
pub const RELEASES: [Release; 5] = [WINDOWS_X64, WINDOWS_ARM64, LINUX_X64, LINUX_ARM64, MACOS];

/// The archive for this computer, or `None` where Godot publishes none.
pub fn release() -> Option<&'static Release> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some(&WINDOWS_X64),
        ("windows", "aarch64") => Some(&WINDOWS_ARM64),
        ("linux", "x86_64") => Some(&LINUX_X64),
        ("linux", "aarch64") => Some(&LINUX_ARM64),
        ("macos", _) => Some(&MACOS),
        _ => None,
    }
}

/// Where the pinned release lives in Ghost's game folder.
pub fn folder(root: &Path) -> PathBuf {
    root.join("godot").join(VERSION)
}

/// Ghost's own Godot, when it has been fetched.
pub fn installed(root: &Path) -> Option<PathBuf> {
    let program = folder(root).join(release()?.program);
    program.is_file().then_some(program)
}

/// A version as Godot prints it ("4.7.2.stable.official.ed1daf0bf") or as a project states it
/// ("4.7"): its major and minor.
pub fn major_minor(version: &str) -> Option<(u8, u8)> {
    let mut parts = version.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// A Godot found on this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub program: PathBuf,
    /// As it prints it, cut to its numbers: "4.7.2".
    pub version: String,
    /// Ghost fetched it, as opposed to the person having installed it.
    pub managed: bool,
}

/// What a Godot says its version is, or `None` when it does not answer like one in ten seconds.
pub async fn version_of(program: &Path) -> Option<String> {
    let mut command = crate::command(program);
    command
        .arg("--version")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(10), command.output())
        .await
        .ok()?
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().rev().find(|line| !line.trim().is_empty())?;
    let numbers: Vec<&str> = line
        .trim()
        .split('.')
        .take_while(|part| part.chars().all(|ch| ch.is_ascii_digit()) && !part.is_empty())
        .collect();
    (numbers.len() >= 2).then(|| numbers.join("."))
}

/// The places a Godot a person installed is usually found, after the `PATH`.
fn usual_places() -> Vec<PathBuf> {
    let mut places = Vec::new();
    if cfg!(target_os = "macos") {
        places.push(PathBuf::from(
            "/Applications/Godot.app/Contents/MacOS/Godot",
        ));
    }
    if cfg!(windows) {
        for root in ["ProgramFiles(x86)", "ProgramFiles"] {
            if let Some(base) = std::env::var_os(root) {
                places.push(
                    PathBuf::from(base)
                        .join("Steam")
                        .join("steamapps")
                        .join("common")
                        .join("Godot Engine")
                        .join("godot.windows.opt.tools.64.exe"),
                );
            }
        }
    }
    places
}

fn on_path() -> Vec<PathBuf> {
    let Some(path) = std::env::var_os("PATH") else {
        return Vec::new();
    };
    let names: &[&str] = if cfg!(windows) {
        &["godot.exe", "godot4.exe"]
    } else {
        &["godot", "godot4"]
    };
    std::env::split_paths(&path)
        .flat_map(|folder| names.iter().map(move |name| folder.join(name)))
        .filter(|program| program.is_file())
        .take(8)
        .collect()
}

/// Every Godot on this computer new enough to run a game for Ghost: its own first, then the
/// person's. Each is asked its version, so this takes a moment.
pub async fn find(root: &Path) -> Vec<Found> {
    let mut found: Vec<Found> = Vec::new();
    if let Some(program) = installed(root) {
        found.push(Found {
            program,
            version: VERSION.to_owned(),
            managed: true,
        });
    }
    for program in on_path().into_iter().chain(usual_places()) {
        if !program.is_file() || found.iter().any(|known| known.program == program) {
            continue;
        }
        if let Some(version) = version_of(&program).await
            && major_minor(&version).is_some_and(|version| version >= OLDEST && version.0 == 4)
        {
            found.push(Found {
                program,
                version,
                managed: false,
            });
        }
    }
    found
}

/// Which Godot runs a project made with `project_version`, or why none here can.
///
/// One with the project's own minor version is best: it reads the project exactly as its editor
/// wrote it. Otherwise the newest that is not older than the project, since Godot 4 reads
/// projects of earlier 4.x releases. A project newer than every Godot here is not run with an
/// older one.
pub fn choose<'a>(project_version: Option<&str>, found: &'a [Found]) -> Result<&'a Found, String> {
    let wanted = project_version.and_then(major_minor);
    if found.is_empty() {
        return Err("Godot is not on this computer yet. Get it in the Game pane.".into());
    }
    let Some(wanted) = wanted else {
        return Ok(&found[0]);
    };
    if let Some(same) = found
        .iter()
        .find(|godot| major_minor(&godot.version) == Some(wanted))
    {
        return Ok(same);
    }
    found
        .iter()
        .filter(|godot| major_minor(&godot.version).is_some_and(|version| version >= wanted))
        .max_by_key(|godot| major_minor(&godot.version))
        .ok_or_else(|| {
            format!(
                "This project was made with Godot {}.{}, which is newer than the Godot here ({}). Install that Godot and Ghost will use it.",
                wanted.0, wanted.1, found[0].version
            )
        })
}

fn worded(error: DownloadError) -> Error {
    Error::Download(match error {
        DownloadError::Cancelled => return Error::Cancelled,
        DownloadError::Io(error) => return Error::Io(error),
        DownloadError::Timeout => "The connection to GitHub timed out.".into(),
        DownloadError::Connect => {
            "GitHub could not be reached. Check the internet connection.".into()
        }
        DownloadError::Interrupted => "The download from GitHub was interrupted.".into(),
        DownloadError::Status(status) => format!("GitHub answered {status} for Godot {VERSION}."),
        DownloadError::TooLarge => {
            "Godot's download is larger than the release Ghost knows.".into()
        }
        DownloadError::CutShort => "Godot's download was cut short.".into(),
        DownloadError::Mismatch => {
            "Godot's download did not match the checksum Ghost pins, so it was thrown away.".into()
        }
        DownloadError::Refused => "Godot's download address is not one Ghost may use.".into(),
    })
}

/// Fetches the pinned Godot into Ghost's folder unless it is there, and answers its program.
/// `report` hears how far the download is; `cancel` stops it at the next chunk and leaves nothing
/// behind.
pub async fn fetch(
    root: &Path,
    report: &(dyn Fn(Progress) + Send + Sync),
    cancel: &AtomicBool,
) -> Result<PathBuf, Error> {
    if let Some(program) = installed(root) {
        return Ok(program);
    }
    let release = release().ok_or_else(|| {
        Error::Refused("Godot publishes no build for this kind of computer.".into())
    })?;
    let base = root.join("godot");
    tokio::fs::create_dir_all(&base).await?;
    let archive = base.join(format!("download-{}.zip", std::process::id()));
    let staging = base.join(format!("unpacking-{}", std::process::id()));
    let result = async {
        let client = ghost_fetch::client(&HOSTS).map_err(worded)?;
        ghost_fetch::download(
            &client,
            &HOSTS,
            Pin {
                url: release.url,
                size: release.size,
                digest: Digest::Sha512(release.sha512),
            },
            &archive,
            report,
            cancel,
        )
        .await
        .map_err(worded)?;
        let _ = tokio::fs::remove_dir_all(&staging).await;
        let (from, into) = (archive.clone(), staging.clone());
        tokio::task::spawn_blocking(move || ghost_fetch::unzip(&from, &into, UNPACK_LIMIT))
            .await
            .map_err(|_| Error::Download("Godot's archive could not be unpacked.".into()))?
            .map_err(|error| Error::Download(error.to_string()))?;
        if !staging.join(release.program).is_file() {
            return Err(Error::Download(
                "Godot's archive did not hold Godot.".into(),
            ));
        }
        let destination = folder(root);
        let _ = tokio::fs::remove_dir_all(&destination).await;
        tokio::fs::rename(&staging, &destination).await?;
        Ok(destination.join(release.program))
    }
    .await;
    let _ = tokio::fs::remove_file(&archive).await;
    let _ = tokio::fs::remove_dir_all(&staging).await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pinned_archive_is_one_file_of_the_one_version_at_godots_own_release() {
        for release in RELEASES {
            assert!(
                release.url.starts_with(&format!(
                    "https://github.com/godotengine/godot/releases/download/{VERSION}-stable/Godot_v{VERSION}-stable_"
                )),
                "{}",
                release.url
            );
            assert!(release.url.ends_with(".zip"));
            assert_eq!(release.sha512.len(), 128, "{}", release.url);
            assert!(
                release
                    .sha512
                    .chars()
                    .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
            );
            assert!(release.size > 50_000_000 && release.size < 400_000_000);
            assert!(!release.program.contains(".."));
        }
        // No two platforms share an archive.
        for (index, release) in RELEASES.iter().enumerate() {
            assert!(
                RELEASES[index + 1..]
                    .iter()
                    .all(|other| { other.url != release.url && other.sha512 != release.sha512 })
            );
        }
    }

    #[test]
    fn versions_read_from_what_godot_prints_and_what_projects_state() {
        assert_eq!(major_minor("4.7.2.stable.official.ed1daf0bf"), Some((4, 7)));
        assert_eq!(major_minor("4.7"), Some((4, 7)));
        assert_eq!(major_minor(" 4.10.1 "), Some((4, 10)));
        assert_eq!(major_minor("four"), None);
        assert_eq!(major_minor("4"), None);
    }

    fn godot(version: &str, managed: bool) -> Found {
        Found {
            program: PathBuf::from(format!("/godot/{version}")),
            version: version.into(),
            managed,
        }
    }

    #[test]
    fn a_project_runs_on_its_own_version_else_the_newest_that_is_not_older() {
        let here = [
            godot("4.7.2", true),
            godot("4.5.1", false),
            godot("4.8", false),
        ];
        assert_eq!(choose(Some("4.5"), &here).unwrap().version, "4.5.1");
        assert_eq!(choose(Some("4.7"), &here).unwrap().version, "4.7.2");
        // No 4.6 here: the newest that reads it.
        assert_eq!(choose(Some("4.6"), &here).unwrap().version, "4.8");
        // A project that states no version runs on Ghost's own.
        assert_eq!(choose(None, &here).unwrap().version, "4.7.2");
        assert_eq!(
            choose(Some("not a version"), &here).unwrap().version,
            "4.7.2"
        );
    }

    #[test]
    fn a_project_newer_than_every_godot_here_is_not_run_on_an_older_one() {
        let here = [godot("4.7.2", true)];
        let refused = choose(Some("4.9"), &here).unwrap_err();
        assert!(
            refused.contains("Godot 4.9") && refused.contains("4.7.2"),
            "{refused}"
        );
        let none = choose(Some("4.7"), &[]).unwrap_err();
        assert!(none.contains("not on this computer"), "{none}");
    }

    #[tokio::test]
    async fn nothing_is_fetched_over_what_is_there() {
        let Some(release) = release() else {
            return;
        };
        let root = tempfile::tempdir().unwrap();
        assert_eq!(installed(root.path()), None);
        let program = folder(root.path()).join(release.program);
        std::fs::create_dir_all(program.parent().unwrap()).unwrap();
        std::fs::write(&program, b"godot").unwrap();
        assert_eq!(installed(root.path()), Some(program.clone()));
        let cancel = AtomicBool::new(false);
        assert_eq!(
            fetch(root.path(), &|_| panic!("no download"), &cancel)
                .await
                .unwrap(),
            program
        );
    }
}
