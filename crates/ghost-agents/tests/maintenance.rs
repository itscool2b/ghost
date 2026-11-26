//! Maintenance uses owned fake binaries. No native login, account, installer or paid prompt runs.
#![cfg(test)]
mod common;
use common::Machine;
use ghost_agents::maintenance::MaintenanceError;
use ghost_protocol::agents::AgentAuthStatus;

fn vendor_machine() -> (Machine, std::path::PathBuf) {
    let machine = Machine::with_tools(&[]);
    let bin = machine.home.join(".local/share/claude/versions/fixture");
    machine.install_tool(&bin, "claude");
    (machine, bin)
}

#[tokio::test]
async fn capabilities_do_not_run_authentication_or_updates() {
    let (machine, bin) = vendor_machine();
    let detector = machine.detector(machine.env_with_path(&[&bin], &[]));
    let available = detector.maintenance("claude").await;
    assert!(available.auth_status);
    assert_eq!(available.update_unavailable, None);
    assert!(
        machine
            .runs("claude")
            .iter()
            .all(|run| run["argv"] == serde_json::json!(["--version"]))
    );
    let missing = detector.maintenance("not-installed").await;
    assert!(!missing.auth_status);
    assert!(missing.update_unavailable.is_some());
}

#[tokio::test]
async fn update_ownership_includes_post_update_verification() {
    let (machine, bin) = vendor_machine();
    std::fs::write(machine.fake.join("claude.maintenance"), "verify-gated").unwrap();
    let detector = machine.detector(machine.env_with_path(&[&bin], &[]));
    let worker = detector.clone();
    let update = tokio::spawn(async move { worker.update("claude").await });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !machine.fake.join("verification-ready").exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        detector.update("claude-acp").await,
        Err(MaintenanceError::AlreadyRunning)
    );
    std::fs::write(machine.fake.join("verification-release"), "continue").unwrap();
    assert_eq!(update.await.unwrap(), Ok(()));
    assert!(
        detector
            .cached()
            .into_iter()
            .find(|entry| entry.id == "claude")
            .unwrap()
            .installation
            .is_some()
    );
}

/// Run in terminal after a failed update: the terminal needs an installation, and the failed update
/// dropped the cached one. It used to refuse until someone pressed Refresh, so it never started.
#[tokio::test]
async fn a_failed_update_detects_again_and_a_terminal_starts_without_a_refresh() {
    let (machine, bin) = vendor_machine();
    let detector = machine.detector(machine.env_with_path(&[&bin], &[]));
    detector.detect(ghost_agents::get("claude").unwrap()).await;
    std::fs::write(machine.fake.join("claude.maintenance"), "fail").unwrap();
    assert_eq!(
        detector.update("claude").await,
        Err(MaintenanceError::Failed(Some(1)))
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !detector
            .cached()
            .iter()
            .any(|entry| entry.id == "claude" && entry.installation.is_some())
        {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the failed update detects the agent again");
    let (name, installation) = detector.fresh_installation("claude").await.unwrap();
    assert_eq!(name, "Claude Code");
    assert_eq!(installation.version, "2.1.310");
}

#[tokio::test]
async fn a_vendor_terminal_waits_for_a_running_update() {
    let (machine, bin) = vendor_machine();
    std::fs::write(machine.fake.join("claude.maintenance"), "slow").unwrap();
    let detector = machine.detector(machine.env_with_path(&[&bin], &[]));
    let worker = std::sync::Arc::clone(&detector);
    let update = tokio::spawn(async move { worker.update("claude").await });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !machine
            .runs("claude")
            .iter()
            .any(|run| run["argv"] == serde_json::json!(["update"]))
        {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let refused = detector.fresh_installation("claude-acp").await.unwrap_err();
    assert!(refused.contains("is updating"), "{refused}");
    assert_eq!(update.await.unwrap(), Ok(()));
    assert!(detector.fresh_installation("claude").await.is_ok());
}

#[tokio::test]
async fn status_runs_the_exact_vendor_command_and_drops_account_details() {
    let (machine, bin) = vendor_machine();
    let detector = machine.detector(machine.env_with_path(&[&bin], &[]));
    assert_eq!(
        detector.auth_status("claude").await,
        AgentAuthStatus::CredentialsPresent
    );
    assert!(
        machine
            .runs("claude")
            .iter()
            .any(|run| run["argv"] == serde_json::json!(["auth", "status", "--json"]))
    );
    std::fs::write(machine.fake.join("claude.maintenance"), "fail").unwrap();
    let status = detector.auth_status("claude").await;
    assert!(matches!(status, AgentAuthStatus::Unknown { .. }));
    assert!(!format!("{status:?}").contains("private"));
}

#[tokio::test]
async fn vendor_update_serializes_aliases_and_rechecks_the_resulting_installation() {
    let (machine, bin) = vendor_machine();
    machine.install_tool(&machine.bin, "node");
    machine.install_tool(&machine.bin, "npm");
    std::fs::write(machine.fake.join("claude.maintenance"), "slow").unwrap();
    let detector = machine.detector(machine.env_with_path(&[&bin, &machine.bin], &[]));
    let alias = ghost_agents::get("claude-acp").unwrap();
    detector.detect(alias).await;
    let worker = std::sync::Arc::clone(&detector);
    let update = tokio::spawn(async move { worker.update("claude").await });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !machine
            .runs("claude")
            .iter()
            .any(|run| run["argv"] == serde_json::json!(["update"]))
        {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        detector.update("claude-acp").await,
        Err(MaintenanceError::AlreadyRunning)
    );
    assert_eq!(update.await.unwrap(), Ok(()));
    let entry = detector
        .cached()
        .into_iter()
        .find(|entry| entry.id == "claude")
        .unwrap();
    assert_eq!(entry.installation.unwrap().version, "2.1.310");
    // An alias's completed probe must not survive the update's cache invalidation.
    let before = machine.runs("claude").len();
    detector.detect(alias).await;
    assert!(machine.runs("claude").len() > before);
    std::fs::write(machine.fake.join("claude.maintenance"), "break-version").unwrap();
    assert_eq!(
        detector.update("claude").await,
        Err(MaintenanceError::VerificationFailed)
    );
}
