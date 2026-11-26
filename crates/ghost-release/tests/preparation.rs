//! Synthetic bytes and a public test key only. No installer or release key is used here.
#![allow(clippy::unwrap_used)]
use std::{fs, path::Path};

use ghost_release::{Artifact, ArtifactKind, Channel, ReleasePlan, prepare, read_plan, verify};
use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/signatures.json")).unwrap()
}

fn plan(root: &Path) -> ReleasePlan {
    fs::create_dir(root).unwrap();
    let fixture = fixture();
    let artifacts = [
        (ArtifactKind::MacosUniversalDmg, "ghost-universal.dmg"),
        (ArtifactKind::MacosUniversalUpdater, "Ghost.app.tar.gz"),
        (ArtifactKind::WindowsX64Nsis, "ghost-x64.exe"),
        (ArtifactKind::WindowsArm64Nsis, "ghost-arm64.exe"),
        (ArtifactKind::LinuxX64Appimage, "auracode-x86_64.AppImage"),
        (ArtifactKind::LinuxX64Deb, "auracode-amd64.deb"),
        (ArtifactKind::LinuxX64Rpm, "auracode-x86_64.rpm"),
    ]
    .into_iter()
    .map(|(kind, file)| {
        fs::write(root.join(file), fixture["artifact"].as_str().unwrap()).unwrap();
        let signature = (kind != ArtifactKind::MacosUniversalDmg).then(|| {
            let name = format!("{file}.sig");
            fs::write(root.join(&name), fixture["signature"].as_str().unwrap()).unwrap();
            name
        });
        Artifact {
            kind,
            file: file.into(),
            signature,
        }
    })
    .collect();
    ReleasePlan {
        channel: Channel::Stable,
        version: "0.1.0".into(),
        pub_date: "2026-10-26T12:00:00Z".into(),
        notes: "Synthetic test release.\nNot an installer.".into(),
        public_key: fixture["public_key"].as_str().unwrap().into(),
        publisher_signed: false,
        artifacts,
    }
}

#[test]
fn staging_manifests_have_a_separate_prefix_and_visible_channel() {
    let temp = tempfile::tempdir().unwrap();
    let artifacts = temp.path().join("artifacts");
    let mut plan = plan(&artifacts);
    plan.channel = Channel::Staging;
    let stage = temp.path().join("stage");
    prepare(&plan, &artifacts, &stage).unwrap();
    verify(&stage, &plan.public_key).unwrap();
    let catalog: Value =
        serde_json::from_slice(&fs::read(stage.join("downloads.json")).unwrap()).unwrap();
    assert_eq!(catalog["release"]["channel"], "staging");
    assert!(
        catalog["release"]["checksums_url"]
            .as_str()
            .unwrap()
            .contains("/releases/download/staging-v0.1.0/")
    );
    let latest = fs::read_to_string(stage.join("latest.json")).unwrap();
    assert!(!latest.contains("/releases/download/v0.1.0/"));
    assert!(latest.contains("/releases/download/staging-v0.1.0/"));
}

#[test]
fn complete_stage_has_exact_target_urls_and_no_private_paths() {
    let temp = tempfile::tempdir().unwrap();
    let artifacts = temp.path().join("artifacts");
    let plan = plan(&artifacts);
    let stage = temp.path().join("stage");
    prepare(&plan, &artifacts, &stage).unwrap();
    verify(&stage, &plan.public_key).unwrap();
    let latest: Value =
        serde_json::from_slice(&fs::read(stage.join("latest.json")).unwrap()).unwrap();
    assert_eq!(latest["platforms"].as_object().unwrap().len(), 10);
    assert_eq!(
        latest["platforms"]["darwin-aarch64"],
        latest["platforms"]["darwin-x86_64"]
    );
    assert_eq!(
        latest["platforms"]["windows-aarch64-nsis"]["url"],
        "https://github.com/itscool2b/auracode/releases/download/v0.1.0/ghost-arm64.exe"
    );
    assert_eq!(
        latest["platforms"]["linux-x86_64"],
        latest["platforms"]["linux-x86_64-appimage"]
    );
    assert_eq!(
        latest["platforms"]["linux-x86_64-rpm"]["url"],
        "https://github.com/itscool2b/auracode/releases/download/v0.1.0/auracode-x86_64.rpm"
    );
    let catalog = fs::read_to_string(stage.join("downloads.json")).unwrap();
    assert!(!catalog.contains(&temp.path().to_string_lossy().to_string()));
    let catalog: Value = serde_json::from_str(&catalog).unwrap();
    assert_eq!(catalog["release"]["downloads"].as_array().unwrap().len(), 6);
    // Updater-signed, so a copy updates itself; no publisher signed it, so each system warns.
    assert_eq!(catalog["release"]["signed"], true);
    assert_eq!(catalog["release"]["publisher_signed"], false);
    assert_eq!(
        catalog["release"]["checksums_url"],
        "https://github.com/itscool2b/auracode/releases/download/v0.1.0/SHA256SUMS"
    );
    assert_eq!(
        catalog["release"]["downloads"][0]["sha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    assert!(!stage.join("PREPARATION_INCOMPLETE").exists());
    // Later changes to inputs cannot replace the signed bytes already staged.
    fs::write(artifacts.join("ghost-x64.exe"), b"changed source").unwrap();
    verify(&stage, &plan.public_key).unwrap();
}

#[test]
fn verification_requires_an_independently_pinned_key() {
    let temp = tempfile::tempdir().unwrap();
    let artifacts = temp.path().join("artifacts");
    let plan = plan(&artifacts);
    let stage = temp.path().join("stage");
    prepare(&plan, &artifacts, &stage).unwrap();
    for expected in ["", "  ", "different-trusted-release-key"] {
        let error = verify(&stage, expected).unwrap_err().to_string();
        assert!(error.contains("independently pinned updater public key"));
    }
    verify(&stage, &plan.public_key).unwrap();

    // The CLI must never silently use the key from the stage it is checking.
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ghost-release"))
        .args(["verify", stage.to_str().unwrap()])
        .env_remove("GHOST_UPDATER_PUBLIC_KEY")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("trusted release configuration"));
    assert!(!error.contains(&plan.public_key));
}

#[test]
fn artifact_order_does_not_change_public_manifests() {
    let temp = tempfile::tempdir().unwrap();
    let artifacts = temp.path().join("artifacts");
    let mut plan = plan(&artifacts);
    prepare(&plan, &artifacts, &temp.path().join("a")).unwrap();
    plan.artifacts.reverse();
    prepare(&plan, &artifacts, &temp.path().join("b")).unwrap();
    for name in ["latest.json", "downloads.json", "SHA256SUMS"] {
        assert_eq!(
            fs::read(temp.path().join("a").join(name)).unwrap(),
            fs::read(temp.path().join("b").join(name)).unwrap()
        );
    }
}

#[test]
fn signatures_reject_modified_bytes_wrong_version_missing_version_and_duplicate_version() {
    for invalid in [
        "bytes",
        "old_signature",
        "unversioned_signature",
        "duplicate_version_signature",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let artifacts = temp.path().join("artifacts");
        let plan = plan(&artifacts);
        if invalid == "bytes" {
            fs::write(artifacts.join("ghost-x64.exe"), b"tampered bytes").unwrap();
        } else {
            fs::write(
                artifacts.join("ghost-x64.exe.sig"),
                fixture()[invalid].as_str().unwrap(),
            )
            .unwrap();
        }
        let stage = temp.path().join("stage");
        assert!(prepare(&plan, &artifacts, &stage).is_err(), "{invalid}");
        assert!(stage.join("PREPARATION_INCOMPLETE").exists());
        assert!(!stage.join("COMPLETE").exists());
        assert!(!stage.join("latest.json").exists());
        assert!(verify(&stage, &plan.public_key).is_err());
    }
}

#[test]
fn existing_stage_is_never_overwritten_and_missing_artifacts_never_complete() {
    let temp = tempfile::tempdir().unwrap();
    let artifacts = temp.path().join("artifacts");
    let plan = plan(&artifacts);
    let stage = temp.path().join("stage");
    fs::create_dir(&stage).unwrap();
    fs::write(stage.join("user-data"), b"keep").unwrap();
    assert!(prepare(&plan, &artifacts, &stage).is_err());
    assert_eq!(fs::read(stage.join("user-data")).unwrap(), b"keep");
    fs::remove_file(artifacts.join("ghost-arm64.exe")).unwrap();
    let incomplete = temp.path().join("incomplete");
    assert!(prepare(&plan, &artifacts, &incomplete).is_err());
    assert!(!incomplete.join("latest.json").exists());
}

#[test]
fn malformed_metadata_and_paths_fail_before_creating_output() {
    let temp = tempfile::tempdir().unwrap();
    let artifacts = temp.path().join("artifacts");
    let original = plan(&artifacts);
    for mutation in 0..9 {
        let mut plan = original.clone();
        match mutation {
            0 => {
                plan.artifacts.pop();
            }
            1 => {
                plan.artifacts[3].kind = ArtifactKind::WindowsX64Nsis;
            }
            2 => {
                plan.artifacts[0].file = "../user.dmg".into();
            }
            3 => {
                plan.artifacts[1].signature = None;
            }
            4 => {
                plan.version = "0.1.0+build".into();
            }
            5 => {
                plan.pub_date = "yesterday".into();
            }
            6 => {
                plan.public_key = "not a key".into();
            }
            7 => {
                plan.artifacts[0].file = "NUL.dmg".into();
            }
            8 => {
                plan.artifacts[0].file = "COM9.dmg".into();
            }
            _ => unreachable!(),
        }
        let stage = temp.path().join(format!("stage-{mutation}"));
        assert!(prepare(&plan, &artifacts, &stage).is_err());
        assert!(!stage.exists());
    }
}

#[test]
fn verification_detects_staged_payload_and_manifest_changes() {
    for file in [
        "latest.json",
        "downloads.json",
        "SHA256SUMS",
        "releases/0.1.0/ghost-x64.exe",
        "releases/0.1.0/ghost-universal.dmg",
        "releases/0.1.0/SHA256SUMS",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let artifacts = temp.path().join("artifacts");
        let plan = plan(&artifacts);
        let stage = temp.path().join("stage");
        prepare(&plan, &artifacts, &stage).unwrap();
        fs::write(stage.join(file), b"changed").unwrap();
        assert!(verify(&stage, &plan.public_key).is_err(), "{file}");
    }
}

#[test]
fn plan_loader_rejects_unknown_fields_and_oversized_input() {
    let temp = tempfile::tempdir().unwrap();
    let artifacts = temp.path().join("artifacts");
    let plan = plan(&artifacts);
    let path = temp.path().join("plan.json");
    fs::write(&path, serde_json::to_vec(&plan).unwrap()).unwrap();
    assert!(read_plan(&path).is_ok());
    let mut value = serde_json::to_value(&plan).unwrap();
    value["private_key"] = "never accepted".into();
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(read_plan(&path).is_err());
    fs::write(&path, vec![b' '; 65537]).unwrap();
    assert!(read_plan(&path).is_err());
}

#[test]
fn public_website_types_match_the_rust_contract() {
    let committed = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/web/app/release-types.ts"),
    )
    .unwrap();
    assert_eq!(
        committed.replace("\r\n", "\n"),
        ghost_release::typescript().unwrap().replace("\r\n", "\n")
    );
}

#[cfg(unix)]
#[test]
fn artifact_links_cannot_read_outside_the_explicit_directory() {
    let temp = tempfile::tempdir().unwrap();
    let artifacts = temp.path().join("artifacts");
    let plan = plan(&artifacts);
    let source = artifacts.join("ghost-universal.dmg");
    fs::remove_file(&source).unwrap();
    let outside = temp.path().join("outside");
    fs::write(&outside, b"private").unwrap();
    std::os::unix::fs::symlink(&outside, &source).unwrap();
    assert!(prepare(&plan, &artifacts, &temp.path().join("stage")).is_err());
}
