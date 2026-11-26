//! Offline, fail-closed release assembly. No network, private keys or platform signing operations.
//! Signatures follow the pinned tauri-plugin-updater 2.14.0 version-binding contract.
//!
//! A release is the files of one GitHub release on the repository: the installers, the updater
//! packages with their signatures, `SHA256SUMS` and `latest.json`, the manifest every installed
//! copy reads to learn of a newer version. This crate checks every byte and signature and writes
//! the manifests; the Publish workflow uploads what it wrote.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
};

use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
use minisign_verify::{PublicKey, Signature};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_PLAN: u64 = 64 * 1024;
const MAX_SIGNATURE: u64 = 4096;
const MAX_ARTIFACT: u64 = 512 * 1024 * 1024;
/// Where a GitHub release serves its files: `<this>/<tag>/<file>`. The repository is
/// `itscool2b/ghost`, but until 19 November 2026 it was `itscool2b/auracode`, and a copy from then
/// accepts update packages under that path alone (`apps/desktop/src-tauri/src/updater.rs`). So the
/// feed names the earlier path, which GitHub redirects to the repository, and copies from after
/// accept both. Switch to `itscool2b/ghost` once no copy from before the rename is in use.
const RELEASES: &str = "https://github.com/itscool2b/auracode/releases/download";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleasePlan {
    pub channel: Channel,
    pub version: String,
    pub pub_date: String,
    pub notes: String,
    /// Tauri's base64-encoded public key, never a private signing key.
    pub public_key: String,
    /// Whether a publisher signed the packages for the operating systems (Authenticode,
    /// Developer ID and notarization), which is separate from the updater signatures checked here.
    /// False unless the plan says so: the download page then says each system will warn.
    #[serde(default)]
    pub publisher_signed: bool,
    pub artifacts: Vec<Artifact>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Stable,
    Staging,
}
impl Channel {
    /// The release's tag. Stable copies read the newest `v` release; a staging copy reads the
    /// `staging` release's manifest, whose packages sit under `staging-v` tags, so neither can be
    /// handed the other's packages.
    pub fn tag(self, version: &str) -> String {
        match self {
            Self::Stable => format!("v{version}"),
            Self::Staging => format!("staging-v{version}"),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub kind: ArtifactKind,
    /// One portable filename under the supplied artifact directory.
    pub file: String,
    /// Detached Tauri .sig filename. Required for updater packages, absent for the DMG.
    pub signature: Option<String>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize, specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    MacosUniversalDmg,
    MacosUniversalUpdater,
    WindowsX64Nsis,
    WindowsArm64Nsis,
    LinuxX64Appimage,
    LinuxX64Deb,
    LinuxX64Rpm,
}

impl ArtifactKind {
    fn extension(self) -> &'static str {
        match self {
            Self::MacosUniversalDmg => ".dmg",
            Self::MacosUniversalUpdater => ".app.tar.gz",
            Self::WindowsX64Nsis | Self::WindowsArm64Nsis => ".exe",
            Self::LinuxX64Appimage => ".AppImage",
            Self::LinuxX64Deb => ".deb",
            Self::LinuxX64Rpm => ".rpm",
        }
    }
    fn targets(self) -> &'static [&'static str] {
        match self {
            Self::MacosUniversalDmg => &[],
            Self::MacosUniversalUpdater => &["darwin-aarch64", "darwin-x86_64"],
            Self::WindowsX64Nsis => &["windows-x86_64-nsis", "windows-x86_64"],
            Self::WindowsArm64Nsis => &["windows-aarch64-nsis", "windows-aarch64"],
            // The plain Linux key is the AppImage's, as the Tauri updater reads it; a deb or rpm
            // install asks for its own key.
            Self::LinuxX64Appimage => &["linux-x86_64-appimage", "linux-x86_64"],
            Self::LinuxX64Deb => &["linux-x86_64-deb"],
            Self::LinuxX64Rpm => &["linux-x86_64-rpm"],
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::MacosUniversalDmg => "macOS · Apple silicon and Intel",
            Self::MacosUniversalUpdater => "macOS updater",
            Self::WindowsX64Nsis => "Windows · x64",
            Self::WindowsArm64Nsis => "Windows · ARM64",
            Self::LinuxX64Appimage => "Linux · AppImage",
            Self::LinuxX64Deb => "Linux · Debian and Ubuntu",
            Self::LinuxX64Rpm => "Linux · Fedora and other RPM systems",
        }
    }
    fn requirements(self) -> &'static str {
        match self {
            Self::MacosUniversalDmg | Self::MacosUniversalUpdater => "macOS 13.3 or newer",
            Self::WindowsX64Nsis | Self::WindowsArm64Nsis => {
                "Windows 10 or newer, with Microsoft Edge WebView2"
            }
            Self::LinuxX64Appimage | Self::LinuxX64Deb | Self::LinuxX64Rpm => {
                "x64 Linux with glibc 2.35 or newer and WebKitGTK 4.1: Ubuntu 22.04, Debian 12, Fedora 36 or newer"
            }
        }
    }
}

#[derive(Serialize)]
struct UpdaterEntry {
    url: String,
    signature: String,
}
#[derive(Serialize)]
struct Latest<'a> {
    version: &'a str,
    pub_date: &'a str,
    notes: &'a str,
    platforms: BTreeMap<String, UpdaterEntry>,
}
#[derive(Serialize, specta::Type)]
struct Download {
    kind: ArtifactKind,
    label: &'static str,
    requirements: &'static str,
    file: String,
    url: String,
    sha256: String,
    bytes: u32,
}
#[derive(Serialize, specta::Type)]
struct CatalogRelease<'a> {
    channel: Channel,
    /// Whether the packages carry verified updater signatures, so an installed copy updates
    /// itself. Every release this crate prepares does.
    signed: bool,
    /// Whether a publisher signed the packages for the operating systems. When not, the download
    /// page says each system warns before one runs, and how to open it.
    publisher_signed: bool,
    version: &'a str,
    pub_date: &'a str,
    notes: &'a str,
    checksums_url: String,
    downloads: Vec<Download>,
}
#[derive(Serialize, specta::Type)]
struct Catalog<'a> {
    release: Option<CatalogRelease<'a>>,
}

/// The website consumes only this generated public data contract.
pub fn typescript() -> Result<String> {
    let types = specta::Types::default().register::<Catalog>();
    Ok(specta_typescript::Typescript::default()
        .header("// Generated by cargo run -p ghost-release -- export-types. Do not edit.\n// biome-ignore-all format: generated type definitions")
        .export(&types, specta_serde::Format)?)
}

fn portable_name(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let numbered_device = stem.len() == 4
        && (stem.starts_with("COM") || stem.starts_with("LPT"))
        && stem.as_bytes()[3].is_ascii_digit();
    !name.is_empty()
        && name.len() <= 160
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        && !name.ends_with('.')
        && !name.contains("..")
        && !["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        && !numbered_device
}

/// The checks every catalog shares, signed or not: its version, date and notes.
fn validate_description(version: &str, pub_date: &str, notes: &str) -> Result<()> {
    let parsed = semver::Version::parse(version).context("Release version must be SemVer")?;
    ensure!(
        parsed.to_string() == version && parsed.build.is_empty(),
        "Use canonical SemVer without build metadata"
    );
    time::OffsetDateTime::parse(pub_date, &time::format_description::well_known::Rfc3339)
        .context("Release date must be RFC3339")?;
    ensure!(
        !notes.trim().is_empty() && notes.len() <= 16 * 1024,
        "Release notes must contain 1 to 16384 bytes"
    );
    Ok(())
}

fn validate(plan: &ReleasePlan) -> Result<PublicKey> {
    validate_description(&plan.version, &plan.pub_date, &plan.notes)?;
    ensure!(
        plan.artifacts.len() == 7,
        "Exactly seven artifacts are required: universal DMG and updater, x64 NSIS, ARM64 NSIS, and the Linux AppImage, deb and rpm"
    );
    let mut kinds = BTreeSet::new();
    let mut names = BTreeSet::new();
    for artifact in &plan.artifacts {
        ensure!(kinds.insert(artifact.kind), "Duplicate artifact kind");
        ensure!(
            portable_name(&artifact.file) && artifact.file.ends_with(artifact.kind.extension()),
            "Invalid artifact filename or extension"
        );
        ensure!(
            names.insert(artifact.file.to_ascii_lowercase()),
            "Artifact filename collision"
        );
        match (&artifact.signature, artifact.kind.targets().is_empty()) {
            (None, true) => {}
            (Some(signature), false) => {
                ensure!(
                    portable_name(signature) && signature == &format!("{}.sig", artifact.file),
                    "Signature must be the artifact filename followed by .sig"
                );
                ensure!(
                    names.insert(signature.to_ascii_lowercase()),
                    "Signature filename collision"
                );
            }
            _ => bail!(
                "Updater packages require signatures; the DMG uses its checksum and separate platform qualification"
            ),
        }
    }
    ensure!(
        plan.public_key.len() <= MAX_SIGNATURE as usize,
        "Public key is too long"
    );
    let decoded = STANDARD
        .decode(plan.public_key.trim())
        .context("Invalid public key encoding")?;
    PublicKey::decode(std::str::from_utf8(&decoded).context("Invalid public key text")?)
        .context("Invalid updater public key")
}

fn open_regular(root: &Path, name: &str, limit: u64) -> Result<File> {
    ensure!(portable_name(name), "Expected a single portable filename");
    let path = root.join(name);
    let metadata = fs::symlink_metadata(&path).context("Artifact is missing")?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Artifact must be a regular file, not a link"
    );
    ensure!(
        fs::canonicalize(&path)?.parent() == Some(root),
        "Artifact resolves outside its directory"
    );
    let directory = cap_std::fs::Dir::open_ambient_dir(root, cap_std::ambient_authority())?;
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No).nonblock(true);
    let file = directory.open_with(name, &options)?.into_std();
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() > 0 && metadata.len() <= limit,
        "Artifact is empty or exceeds its size limit"
    );
    Ok(file)
}

fn bounded(root: &Path, name: &str, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    open_regular(root, name, limit)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "File grew beyond its size limit"
    );
    Ok(bytes)
}

pub fn read_plan(path: &Path) -> Result<ReleasePlan> {
    let parent = fs::canonicalize(
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )?;
    let name = path
        .file_name()
        .and_then(|v| v.to_str())
        .context("Invalid plan filename")?;
    let plan = serde_json::from_slice(&bounded(&parent, name, MAX_PLAN)?)?;
    validate(&plan)?;
    Ok(plan)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn json(value: &impl Serialize) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    ensure!(
        bytes.len() <= MAX_PLAN as usize,
        "Generated manifest exceeds the client metadata limit"
    );
    Ok(bytes)
}

/// Returns deterministic manifests, verifying the exact bytes read. Optional copy uses those same
/// bytes, so later source changes cannot silently replace the artifact whose signature was checked.
fn assemble(
    plan: &ReleasePlan,
    source: &Path,
    copy_to: Option<&Path>,
) -> Result<BTreeMap<String, Vec<u8>>> {
    let key = validate(plan)?;
    let mut platforms = BTreeMap::new();
    let mut downloads = Vec::new();
    let mut checksums = BTreeMap::new();
    let mut artifacts = plan.artifacts.iter().collect::<Vec<_>>();
    artifacts.sort_by_key(|artifact| artifact.kind);
    for artifact in artifacts {
        let encoded = artifact
            .signature
            .as_ref()
            .map(|name| -> Result<_> {
                let bytes = bounded(source, name, MAX_SIGNATURE)?;
                let encoded = std::str::from_utf8(&bytes)?.trim().to_owned();
                let decoded = STANDARD
                    .decode(&encoded)
                    .context("Invalid signature encoding")?;
                let signature = Signature::decode(std::str::from_utf8(&decoded)?)?;
                Ok((name, encoded, signature, bytes))
            })
            .transpose()?;
        // Modern Tauri signatures are prehashed. Refuse legacy unbounded verification paths.
        let mut verifier = encoded
            .as_ref()
            .map(|(_, _, signature, _)| key.verify_stream(signature))
            .transpose()
            .context("Updater signature is not a supported prehashed signature")?;
        let mut input = open_regular(source, &artifact.file, MAX_ARTIFACT)?;
        let mut output = copy_to
            .map(|root| {
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(root.join(&artifact.file))
            })
            .transpose()?;
        let mut hash = Sha256::new();
        let mut total = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = input.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            total += read as u64;
            ensure!(total <= MAX_ARTIFACT, "Artifact grew beyond the size limit");
            hash.update(&buffer[..read]);
            if let Some(verifier) = &mut verifier {
                verifier.update(&buffer[..read]);
            }
            if let Some(output) = &mut output {
                output.write_all(&buffer[..read])?;
            }
        }
        ensure!(total > 0, "Artifact became empty");
        if let Some(mut verifier) = verifier {
            verifier
                .finalize()
                .context("Updater signature verification failed")?;
        }
        if let Some(output) = output {
            output.sync_all()?;
        }
        let url = format!(
            "{RELEASES}/{}/{}",
            plan.channel.tag(&plan.version),
            artifact.file
        );
        let digest = format!("{:x}", hash.finalize());
        checksums.insert(artifact.file.clone(), digest.clone());
        if let Some((name, encoded, signature, bytes)) = encoded {
            let versions = signature
                .trusted_comment()
                .split('\t')
                .filter_map(|field| field.strip_prefix("version:"))
                .collect::<Vec<_>>();
            ensure!(
                versions.len() == 1
                    && semver::Version::parse(versions[0].trim_start_matches('v')).ok()
                        == semver::Version::parse(&plan.version).ok(),
                "Signature is missing the exact advertised app version"
            );
            if let Some(root) = copy_to {
                write_new(&root.join(name), &bytes)?;
            }
            checksums.insert(name.clone(), format!("{:x}", Sha256::digest(&bytes)));
            for target in artifact.kind.targets() {
                platforms.insert(
                    (*target).to_owned(),
                    UpdaterEntry {
                        url: url.clone(),
                        signature: encoded.clone(),
                    },
                );
            }
        }
        if artifact.kind != ArtifactKind::MacosUniversalUpdater {
            downloads.push(Download {
                kind: artifact.kind,
                label: artifact.kind.label(),
                requirements: artifact.kind.requirements(),
                file: artifact.file.clone(),
                url,
                sha256: digest,
                bytes: u32::try_from(total)?,
            });
        }
    }
    let checksum_text = checksums
        .iter()
        .map(|(file, hash)| format!("{hash}  {file}\n"))
        .collect::<String>();
    Ok(BTreeMap::from([
        (
            "latest.json".into(),
            json(&Latest {
                version: &plan.version,
                pub_date: &plan.pub_date,
                notes: &plan.notes,
                platforms,
            })?,
        ),
        (
            "downloads.json".into(),
            json(&Catalog {
                release: Some(CatalogRelease {
                    channel: plan.channel,
                    signed: true,
                    publisher_signed: plan.publisher_signed,
                    version: &plan.version,
                    pub_date: &plan.pub_date,
                    notes: &plan.notes,
                    checksums_url: format!(
                        "{RELEASES}/{}/SHA256SUMS",
                        plan.channel.tag(&plan.version)
                    ),
                    downloads,
                }),
            })?,
        ),
        ("SHA256SUMS".into(), checksum_text.into_bytes()),
    ]))
}

/// Creates only a new output directory. Failures retain partial bytes for inspection, with no
/// completion marker. This function never signs, uploads, overwrites an old stage or deletes files.
pub fn prepare(plan: &ReleasePlan, artifacts: &Path, destination: &Path) -> Result<()> {
    validate(plan)?;
    let source = fs::canonicalize(artifacts)?;
    fs::create_dir(destination)
        .context("Output must be a new directory under an existing parent")?;
    write_new(
        &destination.join("PREPARATION_INCOMPLETE"),
        b"Do not publish this directory.\n",
    )?;
    let payload = destination.join("releases").join(&plan.version);
    fs::create_dir_all(&payload)?;
    let manifests = assemble(plan, &source, Some(&payload))?;
    for (name, bytes) in manifests {
        write_new(&destination.join(name), &bytes)?;
    }
    write_new(
        &payload.join("SHA256SUMS"),
        &fs::read(destination.join("SHA256SUMS"))?,
    )?;
    write_new(&destination.join("release-plan.json"), &json(plan)?)?;
    fs::remove_file(destination.join("PREPARATION_INCOMPLETE"))?;
    write_new(&destination.join("COMPLETE"), b"Offline signatures and checksums verified. Platform qualification and publication remain separate.\n")?;
    Ok(())
}

/// Re-verifies staged bytes and every generated manifest before any external publication step.
pub fn verify(destination: &Path, expected_public_key: &str) -> Result<()> {
    ensure!(
        !expected_public_key.trim().is_empty(),
        "An independently pinned updater public key is required"
    );
    let root = fs::canonicalize(destination)?;
    ensure!(
        !root.join("PREPARATION_INCOMPLETE").exists(),
        "Preparation was interrupted"
    );
    bounded(&root, "COMPLETE", 1024)?;
    let plan = read_plan(&root.join("release-plan.json"))?;
    ensure!(
        plan.public_key.trim() == expected_public_key.trim(),
        "Release stage does not match the independently pinned updater public key"
    );
    let payload = fs::canonicalize(root.join("releases").join(&plan.version))?;
    ensure!(
        payload.starts_with(&root),
        "Payload resolves outside the stage"
    );
    let manifests = assemble(&plan, &payload, None)?;
    for (name, expected) in &manifests {
        ensure!(
            &bounded(&root, name, MAX_PLAN)? == expected,
            "Generated {name} no longer matches the verified artifacts"
        );
        if name == "SHA256SUMS" {
            ensure!(
                &bounded(&payload, name, MAX_PLAN)? == expected,
                "Versioned checksums changed"
            );
        }
    }
    Ok(())
}
