#[cfg(test)]
mod common;
use common::*;
use ghost_agents::Detector;
use ghost_process::{EnvResolver, LeftoverRegistry};
use ghost_protocol::session::Availability;
use std::sync::Arc;
use tokio::{runtime::Handle, sync::mpsc};

/// Deletes a fake tool that a probe just ran. Windows can refuse for a moment while the exited
/// process or a scanner still holds the file, so a denied delete is retried for up to 5 s.
fn remove_tool(path: &std::path::Path) -> std::io::Result<()> {
    for _ in 0..50 {
        match std::fs::remove_file(path) {
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            result => return result,
        }
    }
    std::fs::remove_file(path)
}

fn detector(machine: &Machine, env: ghost_process::ResolvedEnv) -> Arc<Detector> {
    Detector::with_preferences(
        EnvResolver::fixed(env),
        LeftoverRegistry::disabled(),
        machine.adapters.clone(),
        Handle::current(),
        machine.root.path().join("preferences.json"),
    )
}

/// The Connection setting survives a restart, beside the executable choices, and only changes
/// which connection a new session uses.
#[tokio::test]
async fn the_connection_setting_persists_beside_the_executable_choices() {
    let machine = Machine::new();
    let env = machine.env(&[]);
    let detector = detector(&machine, env.clone());
    let entries = detector.detect_all(true).await;
    let claude = entries
        .iter()
        .find(|entry| entry.id == "claude")
        .expect("one Claude Code");
    assert!(!entries.iter().any(|entry| entry.id == "claude-acp"));
    let connections = claude.connections.as_ref().expect("two connections");
    assert_eq!(connections.preferred, None, "Automatic by default");
    assert_eq!(connections.active.as_deref(), Some("claude"), "native runs");
    assert_eq!(
        detector
            .choose_connection("claude")
            .await
            .map(|spec| spec.id),
        Ok("claude")
    );
    detector
        .set_connection("claude", Some("claude-acp"))
        .await
        .expect("saved");
    assert!(machine.root.path().join("agent-connections.json").is_file());
    drop(detector);
    let restarted = self::detector(&machine, env);
    let entries = restarted.detect_all(true).await;
    let connections = entries
        .iter()
        .find(|entry| entry.id == "claude")
        .and_then(|entry| entry.connections.clone())
        .expect("two connections");
    assert_eq!(connections.preferred.as_deref(), Some("claude-acp"));
    assert_eq!(connections.active.as_deref(), Some("claude-acp"));
    assert_eq!(
        restarted
            .choose_connection("claude")
            .await
            .map(|spec| spec.id),
        Ok("claude-acp")
    );
    restarted
        .set_connection("claude", None)
        .await
        .expect("Automatic");
    assert_eq!(
        restarted
            .choose_connection("claude")
            .await
            .map(|spec| spec.id),
        Ok("claude")
    );
}

#[tokio::test]
async fn explicit_shadowed_choice_persists_and_never_changes_an_existing_launch() {
    let machine = Machine::new();
    machine.version("claude", "Claude Code 2.1.310");
    let other = machine.root.path().join("other");
    let second = machine.install_tool(&other, "claude");
    let env = machine.env_with_path(&[&machine.bin, &other], &[]);
    let detector = detector(&machine, env.clone());
    let spec = spec("claude");
    let old = detector
        .prepare_launch(&spec, mpsc::unbounded_channel().0)
        .await
        .unwrap();
    assert_eq!(old.program, machine.tool("claude"));
    let list = detector.installations("claude").await.unwrap();
    assert_eq!(list.candidates.len(), 2);
    assert!(list.candidates[0].selected);
    assert!(!list.candidates[1].selected);
    let result = detector
        .select_installation("claude", Some(second.display().to_string()))
        .await
        .unwrap();
    assert!(result.candidates[1].selected);
    let next = detector
        .prepare_launch(&spec, mpsc::unbounded_channel().0)
        .await
        .unwrap();
    assert_eq!(next.program, second);
    assert_eq!(old.program, machine.tool("claude"));
    assert_eq!(
        detector
            .installations("claude-acp")
            .await
            .unwrap()
            .preferred_path,
        Some(second.display().to_string())
    );
    drop(detector);
    let restarted = self::detector(&machine, env);
    restarted.detect_all(true).await;
    assert_eq!(
        restarted
            .prepare_launch(&spec, mpsc::unbounded_channel().0)
            .await
            .unwrap()
            .program,
        second
    );
    remove_tool(&second).unwrap();
    restarted.detect_all(true).await;
    assert!(matches!(
        restarted.detect(&spec).await,
        Availability::Missing { .. }
    ));
    assert!(
        restarted
            .prepare_launch(&spec, mpsc::unbounded_channel().0)
            .await
            .is_err()
    );
    let absent = restarted.installations("claude").await.unwrap();
    assert!(absent.candidates.iter().any(|candidate| candidate.selected
        && matches!(candidate.availability, Availability::Missing { .. })));
    restarted.select_installation("claude", None).await.unwrap();
    assert_eq!(
        restarted
            .prepare_launch(&spec, mpsc::unbounded_channel().0)
            .await
            .unwrap()
            .program,
        machine.tool("claude")
    );
}

#[tokio::test]
async fn broken_candidates_remain_visible_and_cannot_be_selected() {
    let machine = Machine::new();
    machine.version("claude", "Claude Code 2.1.310");
    let other = machine.root.path().join("broken");
    std::fs::create_dir(&other).unwrap();
    let broken = other.join(format!("claude{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(&broken, "not a program").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&broken, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let detector = detector(
        &machine,
        machine.env_with_path(&[&other, &machine.bin], &[]),
    );
    let inventory = detector.installations("claude").await.unwrap();
    assert_eq!(inventory.candidates.len(), 2);
    assert!(matches!(
        inventory.candidates[0].availability,
        Availability::Broken { .. }
    ));
    assert!(
        inventory.candidates[0].selected,
        "a broken first match does not silently fall through"
    );
    assert!(
        detector
            .select_installation("claude", Some(broken.display().to_string()))
            .await
            .is_err()
    );
    assert!(
        detector
            .select_installation("claude", Some(machine.tool("node").display().to_string()))
            .await
            .is_err()
    );
    assert!(!machine.root.path().join("preferences.json").exists());
    detector
        .select_installation("claude", Some(machine.tool("claude").display().to_string()))
        .await
        .unwrap();
    assert!(matches!(
        detector.detect(&spec("claude")).await,
        Availability::Ready { .. }
    ));
}

#[tokio::test]
async fn metadata_refresh_detects_removal_and_addition_without_restarting_running_launches() {
    let machine = Machine::with_tools(&["claude"]);
    machine.version("claude", "Claude Code 2.1.310");
    let detector = detector(&machine, machine.env(&[]));
    detector.detect_all(true).await;
    detector.refresh_changed_installations().await;
    let old = detector
        .prepare_launch(&spec("claude"), mpsc::unbounded_channel().0)
        .await
        .unwrap();
    remove_tool(&machine.tool("claude")).unwrap();
    detector.refresh_changed_installations().await;
    assert!(matches!(
        detector.detect(&spec("claude")).await,
        Availability::Missing { .. }
    ));
    machine.install_tool(&machine.bin, "claude");
    detector.refresh_changed_installations().await;
    assert!(matches!(
        detector.detect(&spec("claude")).await,
        Availability::Ready { .. }
    ));
    assert_eq!(old.program, machine.tool("claude"));
}

#[tokio::test]
async fn damaged_saved_preferences_do_not_silently_launch_a_default() {
    let machine = Machine::new();
    machine.version("claude", "Claude Code 2.1.310");
    std::fs::write(machine.root.path().join("preferences.json"), "invalid").unwrap();
    let detector = detector(&machine, machine.env(&[]));
    assert!(matches!(
        detector.detect(&spec("claude")).await,
        Availability::Missing { .. }
    ));
    assert!(
        detector
            .installations("claude")
            .await
            .unwrap()
            .notice
            .is_some()
    );
    assert!(detector.select_installation("claude", None).await.is_err());
}

#[tokio::test]
async fn a_probe_started_before_selection_cannot_launch_the_old_executable() {
    let machine = Machine::new();
    let old = machine.install_tool(&machine.root.path().join("override"), "custom");
    machine.version("custom", "Claude Code 2.1.310");
    machine.mode("custom", "gated-version");
    let detector = detector(
        &machine,
        machine.env(&[("CLAUDE_CODE_EXECUTABLE", old.to_str().unwrap())]),
    );
    let starting = Arc::clone(&detector);
    let launch = tokio::spawn(async move {
        starting
            .prepare_launch(&spec("claude"), mpsc::unbounded_channel().0)
            .await
    });
    let wait_for = |path: std::path::PathBuf| async move {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !path.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    };
    wait_for(machine.fake.join("verification-ready")).await;
    let choosing = Arc::clone(&detector);
    let selected = machine.tool("claude").display().to_string();
    let change =
        tokio::spawn(async move { choosing.select_installation("claude", Some(selected)).await });
    wait_for(machine.root.path().join("preferences.json")).await;
    // Wait for the selected cache to publish before releasing the old probe. Its return must
    // still be rejected by the configuration generation, even if it succeeds afterwards.
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !detector.cached().iter().any(|entry| entry.id == "claude" && matches!(&entry.availability, Availability::Ready {path,..} if path == &machine.tool("claude").display().to_string())) {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    std::fs::write(machine.fake.join("verification-release"), "release").unwrap();
    change.await.unwrap().unwrap();
    assert_eq!(
        launch.await.unwrap().unwrap().program,
        machine.tool("claude")
    );
}
