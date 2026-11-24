//! Google's Android SDK repository index: which packages exist, where their archives are and
//! what their checksums are. It is the same index `sdkmanager` reads; reading it here means the
//! pane's three packages (platform-tools, the emulator, one system image) need no Java.

use quick_xml::Reader;
use quick_xml::XmlVersion;
use quick_xml::events::{BytesRef, BytesStart, Event};

use crate::Error;

/// Where the index and its archives live. Nothing is fetched from anywhere else.
pub const HOST: &str = "dl.google.com";
/// The main index: platform-tools and the emulator.
pub const MAIN_INDEX: &str = "https://dl.google.com/android/repository/repository2-3.xml";
/// The index of system images with Google's APIs.
pub const IMAGE_INDEX: &str =
    "https://dl.google.com/android/repository/sys-img/google_apis/sys-img2-3.xml";

/// The licence every package o7 fetches is under. A package under any other (the preview
/// licence) is never chosen.
pub const LICENSE: &str = "android-sdk-license";
/// The stable channel.
const STABLE: &str = "channel-0";

/// The operating system an archive is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostOs {
    Windows,
    MacOs,
    Linux,
}

/// The processor an archive is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostArch {
    X64,
    Arm64,
}

/// This computer, as the index names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Host {
    pub os: HostOs,
    pub arch: HostArch,
}

impl Host {
    /// The computer o7 is running on, or `None` where Android's tools are not published.
    pub fn current() -> Option<Host> {
        let os = match std::env::consts::OS {
            "windows" => HostOs::Windows,
            "macos" => HostOs::MacOs,
            "linux" => HostOs::Linux,
            _ => return None,
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => HostArch::X64,
            "aarch64" => HostArch::Arm64,
            _ => return None,
        };
        Some(Host { os, arch })
    }

    /// The processor a system image must be built for to run fast here.
    pub fn image_abi(&self) -> &'static str {
        match self.arch {
            HostArch::X64 => "x86_64",
            HostArch::Arm64 => "arm64-v8a",
        }
    }
}

/// A checksum as the index states it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Checksum {
    Sha1(String),
    Sha256(String),
}

/// One downloadable file of a package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Archive {
    /// The whole address, always under `https://dl.google.com/`.
    pub url: String,
    pub size: u64,
    pub checksum: Checksum,
    os: Option<HostOs>,
    arch: Option<HostArch>,
}

impl Archive {
    fn fits(&self, host: Host) -> bool {
        self.os.is_none_or(|os| os == host.os) && self.arch.is_none_or(|arch| arch == host.arch)
    }
}

/// One package of the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    /// `platform-tools`, `emulator`, `system-images;android-36;google_apis;x86_64`.
    pub path: String,
    /// Major, minor, micro.
    pub revision: (u32, u32, u32),
    pub name: String,
    license: String,
    channel: String,
    obsolete: bool,
    preview: bool,
    archives: Vec<Archive>,
}

impl Package {
    fn stable(&self) -> bool {
        !self.obsolete && !self.preview && self.channel == STABLE && self.license == LICENSE
    }
}

/// An index, parsed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Index {
    pub packages: Vec<Package>,
    /// The text of [`LICENSE`], which the person reads and accepts before anything is fetched.
    pub license: Option<String>,
}

/// A package chosen for this computer, with the one archive to fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub path: String,
    pub name: String,
    pub revision: (u32, u32, u32),
    pub archive: Archive,
}

/// A system image's place in an SDK: `system-images/android-36/google_apis/x86_64`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImagePath {
    /// `android-36`.
    pub platform: String,
    /// `google_apis`.
    pub tag: String,
    /// `x86_64`.
    pub abi: String,
}

impl ImagePath {
    /// Reads `system-images;android-36;google_apis;x86_64`.
    pub fn parse(path: &str) -> Option<ImagePath> {
        let mut parts = path.split(';');
        if parts.next()? != "system-images" {
            return None;
        }
        let image = ImagePath {
            platform: parts.next()?.to_owned(),
            tag: parts.next()?.to_owned(),
            abi: parts.next()?.to_owned(),
        };
        let plain = |part: &str| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
                && !part.starts_with('.')
        };
        (parts.next().is_none() && plain(&image.platform) && plain(&image.tag) && plain(&image.abi))
            .then_some(image)
    }

    /// The Android version as a number that sorts: `android-36` is 36.0, `android-37.0` is 37.0.
    /// `None` for an extension, beta or preview image (`android-36-ext19`, `android-Baklava`).
    pub fn api(&self) -> Option<(u32, u32)> {
        let version = self.platform.strip_prefix("android-")?;
        let (major, minor) = version.split_once('.').unwrap_or((version, "0"));
        Some((major.parse().ok()?, minor.parse().ok()?))
    }

    /// The folder under an SDK's root, with `/` between its parts.
    pub fn folder(&self) -> String {
        format!("system-images/{}/{}/{}", self.platform, self.tag, self.abi)
    }
}

impl Index {
    /// Reads an index document fetched from `source` (archive addresses are relative to it).
    pub fn parse(xml: &str, source: &str) -> Result<Index, Error> {
        let base = source.rsplit_once('/').map_or(source, |(base, _)| base);
        if !base.starts_with(&format!("https://{HOST}/")) {
            return Err(Error::Refused(
                "The index is not from Google's download site.".into(),
            ));
        }
        let unreadable = || Error::Protocol("Google's package index could not be read.".into());
        let mut reader = Reader::from_str(xml);
        let mut path: Vec<String> = Vec::new();
        let mut text = String::new();
        let mut index = Index::default();
        let mut package: Option<Package> = None;
        let mut archive: Option<Draft> = None;
        let mut license: Option<String> = None;
        loop {
            match reader.read_event().map_err(|_| unreadable())? {
                Event::Start(element) => {
                    let name = local(&element);
                    text.clear();
                    match name.as_str() {
                        "license" => license = attribute(&element, "id"),
                        "remotePackage" => {
                            package = Some(Package {
                                path: attribute(&element, "path").unwrap_or_default(),
                                revision: (0, 0, 0),
                                name: String::new(),
                                license: String::new(),
                                channel: String::new(),
                                obsolete: attribute(&element, "obsolete").as_deref()
                                    == Some("true"),
                                preview: false,
                                archives: Vec::new(),
                            });
                        }
                        "archive" => archive = Some(Draft::default()),
                        "checksum" => {
                            if let Some(draft) = &mut archive {
                                draft.checksum_type = attribute(&element, "type");
                            }
                        }
                        _ => {}
                    }
                    path.push(name);
                }
                Event::Empty(element) => {
                    if let Some(package) = &mut package {
                        match local(&element).as_str() {
                            "uses-license" => {
                                package.license = attribute(&element, "ref").unwrap_or_default();
                            }
                            "channelRef" => {
                                package.channel = attribute(&element, "ref").unwrap_or_default();
                            }
                            _ => {}
                        }
                    }
                }
                Event::Text(chunk) => text.push_str(&chunk),
                Event::CData(chunk) => text.push_str(&chunk),
                Event::GeneralRef(reference) => text.push_str(&resolve(&reference)),
                Event::End(_) => {
                    let Some(name) = path.pop() else {
                        return Err(unreadable());
                    };
                    let parent = path.last().map(String::as_str).unwrap_or_default();
                    let value = text.trim();
                    match (name.as_str(), parent) {
                        ("license", _) => {
                            if license.take().as_deref() == Some(LICENSE) {
                                index.license = Some(text.trim().to_owned());
                            }
                        }
                        ("remotePackage", _) => {
                            if let Some(package) = package.take() {
                                index.packages.push(package);
                            }
                        }
                        ("archive", _) => {
                            if let (Some(package), Some(draft)) = (&mut package, archive.take())
                                && let Some(archive) = draft.finish(base)
                            {
                                package.archives.push(archive);
                            }
                        }
                        ("display-name", "remotePackage") => {
                            if let Some(package) = &mut package {
                                package.name = value.to_owned();
                            }
                        }
                        (part, "revision")
                            if path.len() >= 2 && path[path.len() - 2] == "remotePackage" =>
                        {
                            if let Some(package) = &mut package {
                                let number = value.parse().unwrap_or(0);
                                match part {
                                    "major" => package.revision.0 = number,
                                    "minor" => package.revision.1 = number,
                                    "micro" => package.revision.2 = number,
                                    "preview" => package.preview = true,
                                    _ => {}
                                }
                            }
                        }
                        // Only the whole archive: a `patch` beside it has its own size and URL.
                        (part, "complete") => {
                            if let Some(draft) = &mut archive {
                                match part {
                                    "size" => draft.size = value.parse().ok(),
                                    "checksum" => draft.checksum = Some(value.to_ascii_lowercase()),
                                    "url" => draft.url = Some(value.to_owned()),
                                    _ => {}
                                }
                            }
                        }
                        ("host-os", "archive") => {
                            if let Some(draft) = &mut archive {
                                draft.os = Some(value.to_owned());
                            }
                        }
                        ("host-arch", "archive") => {
                            if let Some(draft) = &mut archive {
                                draft.arch = Some(value.to_owned());
                            }
                        }
                        _ => {}
                    }
                    text.clear();
                }
                Event::Eof => break,
                _ => {}
            }
        }
        if index.packages.is_empty() {
            return Err(unreadable());
        }
        Ok(index)
    }

    /// The newest stable release of the package at `path` that has an archive for `host`.
    pub fn newest(&self, path: &str, host: Host) -> Option<Choice> {
        self.packages
            .iter()
            .filter(|package| package.path == path && package.stable())
            .filter_map(|package| choice(package, host))
            .max_by_key(|choice| choice.revision)
    }

    /// The newest stable system image for `host`: a released Android version (not an extension,
    /// beta or 16 KB page-size variant), the highest there is.
    pub fn newest_image(&self, host: Host) -> Option<Choice> {
        let abi = host.image_abi();
        self.packages
            .iter()
            .filter(|package| package.stable())
            .filter_map(|package| {
                let image = ImagePath::parse(&package.path)?;
                let api = image.api()?;
                (image.tag == "google_apis" && image.abi == abi)
                    .then(|| choice(package, host).map(|choice| (api, choice)))
                    .flatten()
            })
            .max_by_key(|(api, choice)| (*api, choice.revision))
            .map(|(_, choice)| choice)
    }
}

fn choice(package: &Package, host: Host) -> Option<Choice> {
    let archive = package
        .archives
        .iter()
        .find(|archive| archive.fits(host))?
        .clone();
    Some(Choice {
        path: package.path.clone(),
        name: package.name.clone(),
        revision: package.revision,
        archive,
    })
}

/// An `<archive>` while it is being read.
#[derive(Debug, Default)]
struct Draft {
    size: Option<u64>,
    checksum: Option<String>,
    checksum_type: Option<String>,
    url: Option<String>,
    os: Option<String>,
    arch: Option<String>,
}

impl Draft {
    /// The archive, or `None` when the entry is incomplete, for a system o7 does not run on, or
    /// would lead anywhere but a plain file beside the index.
    fn finish(self, base: &str) -> Option<Archive> {
        let file = self.url?;
        let plain = !file.is_empty()
            && !file.contains("..")
            && file
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/'))
            && !file.starts_with('/');
        if !plain {
            return None;
        }
        let digest = self.checksum?;
        if !digest.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return None;
        }
        let checksum = match (
            self.checksum_type.as_deref().unwrap_or("sha1"),
            digest.len(),
        ) {
            ("sha1" | "sha-1", 40) => Checksum::Sha1(digest),
            ("sha256" | "sha-256", 64) => Checksum::Sha256(digest),
            _ => return None,
        };
        let os = match self.os.as_deref() {
            None => None,
            Some("windows") => Some(HostOs::Windows),
            Some("macosx") => Some(HostOs::MacOs),
            Some("linux") => Some(HostOs::Linux),
            Some(_) => return None,
        };
        let arch = match self.arch.as_deref() {
            None => None,
            Some("x64") => Some(HostArch::X64),
            Some("aarch64") => Some(HostArch::Arm64),
            Some(_) => return None,
        };
        Some(Archive {
            url: format!("{base}/{file}"),
            size: self.size.filter(|size| *size > 0)?,
            checksum,
            os,
            arch,
        })
    }
}

/// An element's name without its namespace prefix.
fn local(element: &BytesStart<'_>) -> String {
    element.local_name().as_ref().to_owned()
}

fn attribute(element: &BytesStart<'_>, key: &str) -> Option<String> {
    element
        .attributes()
        .flatten()
        .find(|attribute| attribute.key.as_ref() == key)
        .and_then(|attribute| {
            attribute
                .normalized_value(XmlVersion::Implicit1_0)
                .ok()
                .map(|value| value.into_owned())
        })
}

/// `&amp;`, `&#10;` and the like, as the characters they stand for.
fn resolve(reference: &BytesRef<'_>) -> String {
    if let Ok(Some(ch)) = reference.resolve_char_ref() {
        return ch.to_string();
    }
    quick_xml::escape::resolve_predefined_entity(reference)
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOWS: Host = Host {
        os: HostOs::Windows,
        arch: HostArch::X64,
    };
    const MAC_ARM: Host = Host {
        os: HostOs::MacOs,
        arch: HostArch::Arm64,
    };
    const LINUX: Host = Host {
        os: HostOs::Linux,
        arch: HostArch::X64,
    };

    fn archive(
        file: &str,
        size: u64,
        digest: &str,
        os: Option<&str>,
        arch: Option<&str>,
    ) -> String {
        format!(
            "<archive><complete><size>{size}</size><checksum type=\"sha1\">{digest}</checksum><url>{file}</url></complete>\
             <patches><patch><size>1</size><checksum type=\"sha1\">{bad}</checksum><url>patch.zip</url></patch></patches>{os}{arch}</archive>",
            bad = "0".repeat(40),
            os = os
                .map(|os| format!("<host-os>{os}</host-os>"))
                .unwrap_or_default(),
            arch = arch
                .map(|arch| format!("<host-arch>{arch}</host-arch>"))
                .unwrap_or_default(),
        )
    }

    fn package(
        path: &str,
        revision: (u32, u32, u32),
        extra: &str,
        license: &str,
        channel: &str,
        archives: &str,
    ) -> String {
        format!(
            "<remotePackage path=\"{path}\"{extra}><type-details xsi:type=\"x\"><api-level>36</api-level></type-details>\
             <revision><major>{}</major><minor>{}</minor><micro>{}</micro></revision>\
             <display-name>Name of {path}</display-name><uses-license ref=\"{license}\"/>\
             <dependencies><dependency path=\"emulator\"><min-revision><major>99</major></min-revision></dependency></dependencies>\
             <channelRef ref=\"{channel}\"/><archives>{archives}</archives></remotePackage>",
            revision.0, revision.1, revision.2
        )
    }

    fn document(packages: &str) -> String {
        format!(
            "<?xml version='1.0' encoding='utf-8'?><sdk:sdk-repository xmlns:sdk=\"x\">\
             <license id=\"android-sdk-license\" type=\"text\">Terms &amp; Conditions\n\n1. &quot;Android&quot; means the stack.</license>\
             <license id=\"android-sdk-preview-license\" type=\"text\">Preview terms</license>\
             <channel id=\"channel-0\">stable</channel>{packages}</sdk:sdk-repository>"
        )
    }

    fn sha(fill: char) -> String {
        fill.to_string().repeat(40)
    }

    #[test]
    fn the_newest_stable_release_for_this_computer_is_chosen() {
        let xml = document(
            &[
                package(
                    "emulator",
                    (37, 4, 1),
                    "",
                    "android-sdk-preview-license",
                    "channel-2",
                    &archive(
                        "emulator-windows_x64-2.zip",
                        9,
                        &sha('a'),
                        Some("windows"),
                        Some("x64"),
                    ),
                ),
                package(
                    "emulator",
                    (37, 3, 10),
                    "",
                    LICENSE,
                    STABLE,
                    &[
                        archive(
                            "emulator-linux_x64-1.zip",
                            350,
                            &sha('b'),
                            Some("linux"),
                            Some("x64"),
                        ),
                        archive(
                            "emulator-darwin_x64-1.zip",
                            487,
                            &sha('c'),
                            Some("macosx"),
                            Some("x64"),
                        ),
                        archive(
                            "emulator-darwin_aarch64-1.zip",
                            420,
                            &sha('d'),
                            Some("macosx"),
                            Some("aarch64"),
                        ),
                        archive(
                            "emulator-windows_x64-1.zip",
                            456,
                            &sha('e'),
                            Some("windows"),
                            Some("x64"),
                        ),
                    ]
                    .concat(),
                ),
                package(
                    "emulator",
                    (36, 0, 0),
                    "",
                    LICENSE,
                    STABLE,
                    &archive("old.zip", 1, &sha('f'), Some("windows"), Some("x64")),
                ),
                package(
                    "emulator",
                    (99, 0, 0),
                    " obsolete=\"true\"",
                    LICENSE,
                    STABLE,
                    &archive("gone.zip", 1, &sha('f'), Some("windows"), None),
                ),
                package(
                    "platform-tools",
                    (37, 0, 3),
                    "",
                    LICENSE,
                    STABLE,
                    &archive(
                        "platform-tools_r37.0.3-win.zip",
                        8,
                        &sha('1'),
                        Some("windows"),
                        None,
                    ),
                ),
            ]
            .concat(),
        );
        let index = Index::parse(&xml, MAIN_INDEX).unwrap();
        assert_eq!(
            index.license.as_deref(),
            Some("Terms & Conditions\n\n1. \"Android\" means the stack.")
        );
        let emulator = index.newest("emulator", WINDOWS).unwrap();
        assert_eq!(emulator.revision, (37, 3, 10));
        assert_eq!(emulator.name, "Name of emulator");
        assert_eq!(
            emulator.archive.url,
            "https://dl.google.com/android/repository/emulator-windows_x64-1.zip"
        );
        assert_eq!(emulator.archive.size, 456);
        assert_eq!(emulator.archive.checksum, Checksum::Sha1(sha('e')));
        assert!(
            index
                .newest("emulator", MAC_ARM)
                .unwrap()
                .archive
                .url
                .ends_with("darwin_aarch64-1.zip")
        );
        assert!(
            index
                .newest("emulator", LINUX)
                .unwrap()
                .archive
                .url
                .ends_with("linux_x64-1.zip")
        );
        // Nothing is published for Windows on ARM, and platform-tools has no Linux archive here.
        assert!(
            index
                .newest(
                    "emulator",
                    Host {
                        os: HostOs::Windows,
                        arch: HostArch::Arm64
                    }
                )
                .is_none()
        );
        assert!(index.newest("platform-tools", LINUX).is_none());
        assert_eq!(
            index.newest("platform-tools", WINDOWS).unwrap().revision,
            (37, 0, 3)
        );
        assert!(index.newest("build-tools", WINDOWS).is_none());
    }

    #[test]
    fn the_image_is_the_highest_released_android_for_this_processor() {
        let image = |path: &str, revision: u32, license: &str, file: &str| {
            package(
                path,
                (revision, 0, 0),
                "",
                license,
                STABLE,
                &archive(file, 1_900, &sha('9'), None, None),
            )
        };
        let xml = document(
            &[
                image(
                    "system-images;android-35;google_apis;x86_64",
                    9,
                    LICENSE,
                    "x86_64-35_r09.zip",
                ),
                image(
                    "system-images;android-36;google_apis;x86_64",
                    7,
                    LICENSE,
                    "x86_64-36_r07.zip",
                ),
                image(
                    "system-images;android-36;google_apis;arm64-v8a",
                    7,
                    LICENSE,
                    "arm64-v8a-36_r07.zip",
                ),
                image(
                    "system-images;android-37.0;google_apis;x86_64",
                    8,
                    LICENSE,
                    "x86_64-37.0_r08.zip",
                ),
                image(
                    "system-images;android-37.0;google_apis;x86_64",
                    6,
                    LICENSE,
                    "x86_64-37.0_r06.zip",
                ),
                image(
                    "system-images;android-37.2;google_apis_ps16k;x86_64",
                    1,
                    LICENSE,
                    "ps16k.zip",
                ),
                image(
                    "system-images;android-36-ext19;google_apis;x86_64",
                    1,
                    LICENSE,
                    "ext.zip",
                ),
                image(
                    "system-images;android-38;google_apis;x86_64",
                    1,
                    "android-sdk-preview-license",
                    "preview.zip",
                ),
                image(
                    "system-images;android-Baklava;google_apis;x86_64",
                    1,
                    LICENSE,
                    "codename.zip",
                ),
            ]
            .concat(),
        );
        let index = Index::parse(&xml, IMAGE_INDEX).unwrap();
        let chosen = index.newest_image(WINDOWS).unwrap();
        assert_eq!(chosen.path, "system-images;android-37.0;google_apis;x86_64");
        assert_eq!(
            chosen.archive.url,
            "https://dl.google.com/android/repository/sys-img/google_apis/x86_64-37.0_r08.zip"
        );
        assert_eq!(
            index.newest_image(MAC_ARM).unwrap().path,
            "system-images;android-36;google_apis;arm64-v8a"
        );
        let path = ImagePath::parse(&chosen.path).unwrap();
        assert_eq!(
            path.folder(),
            "system-images/android-37.0/google_apis/x86_64"
        );
        assert_eq!(path.api(), Some((37, 0)));
    }

    #[test]
    fn an_archive_that_could_lead_elsewhere_is_left_out() {
        let bad = [
            archive("../../evil.zip", 5, &sha('a'), None, None),
            archive("https://example.com/x.zip", 5, &sha('a'), None, None),
            archive("/abs.zip", 5, &sha('a'), None, None),
            archive("a b.zip", 5, &sha('a'), None, None),
            archive("short.zip", 5, "abc123", None, None),
            archive("zero.zip", 0, &sha('a'), None, None),
            archive("nothex.zip", 5, &"z".repeat(40), None, None),
            archive("bsd.zip", 5, &sha('a'), Some("freebsd"), None),
        ]
        .concat();
        let xml = document(&package(
            "platform-tools",
            (1, 0, 0),
            "",
            LICENSE,
            STABLE,
            &bad,
        ));
        let index = Index::parse(&xml, MAIN_INDEX).unwrap();
        assert!(index.newest("platform-tools", WINDOWS).is_none());
        assert!(
            Index::parse(
                &xml,
                "https://example.com/android/repository/repository2-3.xml"
            )
            .is_err()
        );
        assert!(
            Index::parse(
                &xml,
                "http://dl.google.com/android/repository/repository2-3.xml"
            )
            .is_err()
        );
        assert!(Index::parse("<nothing/>", MAIN_INDEX).is_err());
        assert!(Index::parse("not xml <", MAIN_INDEX).is_err());
    }

    #[test]
    fn image_paths_are_plain_folder_names() {
        assert!(ImagePath::parse("system-images;android-36;google_apis;x86_64").is_some());
        for bad in [
            "platform-tools",
            "system-images;android-36;google_apis",
            "system-images;android-36;google_apis;x86_64;more",
            "system-images;..;google_apis;x86_64",
            "system-images;android-36;google/apis;x86_64",
            "system-images;android-36;;x86_64",
        ] {
            assert!(ImagePath::parse(bad).is_none(), "{bad}");
        }
        assert_eq!(
            ImagePath::parse("system-images;android-36-ext19;google_apis;x86_64")
                .unwrap()
                .api(),
            None
        );
    }
}
