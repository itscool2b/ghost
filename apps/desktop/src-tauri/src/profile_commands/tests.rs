use super::*;
use crate::{glass::GlassEffect, launch::LaunchOptions, os::OsInfo};
use ghost_core::{chatstore::NewChat, owner::ProfileOwner};

fn state(root: &DataRoot) -> AppState {
    let mut booted = boot::boot(Ok(root.clone()));
    let chats = sessions::testing::start(&mut booted);
    AppState::new(
        booted,
        chats,
        &OsInfo {
            os: "windows",
            version: None,
            display: "test".into(),
        },
        GlassEffect::None,
        LaunchOptions::from_lookup(|_| None),
        "0.1.0".into(),
    )
}
async fn add_chat(state: &AppState, folder: &std::path::Path) -> String {
    state
        .chats()
        .unwrap()
        .store
        .create_chat(NewChat {
            agent_id: "mock".into(),
            agent_host: Default::default(),
            folder: folder.into(),
            device_id: state.app_info().device.unwrap().device_id,
            now: 1,
        })
        .await
        .unwrap()
        .id
}
async fn close(state: &AppState) {
    if let Some(chats) = state.chats() {
        discard(&chats).await;
    }
}

#[tokio::test]
async fn switching_keeps_datasets_separate_flushes_and_invalidates_old_channels() {
    let temp = tempfile::tempdir().unwrap();
    let root = DataRoot::at(temp.path().join("data"));
    let state = state(&root);
    let profiles = Profiles::new(root.clone());
    let first = profiles.open_active().unwrap();
    let chat = add_chat(&state, temp.path()).await;
    let generation = state.sessions().unwrap().generation();
    let page = state.pages.for_status("main");
    let next = profiles.create("Another local dataset").unwrap();
    let info = activate(&state, next.clone(), |_, _, _, _| panic!("reuse services"))
        .await
        .unwrap();
    assert_eq!(info.profile.unwrap().id, next.meta.id);
    assert!(!info.profile_switching);
    assert!(!page.is_current());
    assert!(!generation.is_current());
    assert!(!state.pages.for_view("main").is_current());
    assert_eq!(profiles.open_active().unwrap().meta.id, next.meta.id);
    assert!(
        state
            .chats()
            .unwrap()
            .store
            .chats()
            .await
            .unwrap()
            .is_empty()
    );
    activate(&state, first, sessions::testing::outside)
        .await
        .unwrap();
    assert_eq!(
        state.chats().unwrap().store.chats().await.unwrap()[0].id,
        chat
    );
    close(&state).await;
}

#[tokio::test]
async fn admitted_command_drains_before_switch_and_new_commands_are_refused() {
    let temp = tempfile::tempdir().unwrap();
    let root = DataRoot::at(temp.path().join("data"));
    let state = state(&root);
    let next = Profiles::new(root.clone()).create("Next").unwrap();
    let lease = state.sessions().unwrap();
    let switching = activate(&state, next, sessions::testing::outside);
    tokio::pin!(switching);
    tokio::select! {
        result = &mut switching => panic!("must wait for admitted command: {result:?}"),
        _ = tokio::time::sleep(Duration::from_millis(20)) => {}
    }
    assert!(state.sessions().is_err());
    assert!(state.onboarding().is_err());
    drop(lease);
    switching.await.unwrap();
    assert!(state.sessions().is_ok());
    close(&state).await;
}

#[tokio::test]
async fn a_library_command_paused_after_lease_acquisition_cannot_register_during_profile_drain() {
    let temp = tempfile::tempdir().unwrap();
    let root = DataRoot::at(temp.path().join("data"));
    let state = state(&root);
    let next = Profiles::new(root).create("Next").unwrap();
    let lease = state.sessions().unwrap();
    let generation = lease.generation();
    let mut switching = Box::pin(activate(&state, next, sessions::testing::outside));
    // Poll exactly to the held command lease. No timer or scheduling guess releases the command.
    std::future::poll_fn(|cx| {
        assert!(std::future::Future::poll(switching.as_mut(), cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    assert!(state.sessions().is_err());
    assert!(
        generation.is_current(),
        "the old profile token remains current until publication"
    );
    assert!(
        lease
            .plugin_library_progressive(true, |_| {
                panic!("late registration must be rejected before inventory or helper work")
            })
            .await
            .is_err()
    );
    drop(lease);
    switching.await.unwrap();
    assert!(!generation.is_current());
    assert!(
        state
            .sessions()
            .unwrap()
            .plugin_library_progressive(false, |_| false)
            .await
            .is_ok()
    );
    close(&state).await;
}

#[tokio::test]
async fn abandoned_or_failed_profile_drains_reopen_library_admission_on_the_current_profile() {
    let temp = tempfile::tempdir().unwrap();
    let root = DataRoot::at(temp.path().join("data"));
    let state = state(&root);
    let next = Profiles::new(root).create("Next").unwrap();
    let lease = state.sessions().unwrap();
    let generation = lease.generation();
    let mut switching = Box::pin(activate(&state, next.clone(), sessions::testing::outside));
    std::future::poll_fn(|cx| {
        assert!(std::future::Future::poll(switching.as_mut(), cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    assert!(
        lease
            .plugin_library_progressive(false, |_| false)
            .await
            .is_err()
    );
    drop(switching);
    assert!(generation.is_current());
    assert!(!state.app_info().profile_switching);
    assert!(
        lease
            .plugin_library_progressive(false, |_| false)
            .await
            .is_ok(),
        "dropping the switch future reopens the same admitted manager"
    );
    drop(lease);
    let terminal = state.terminals.reserve("main").unwrap();
    assert!(
        activate(&state, next, sessions::testing::outside)
            .await
            .is_err()
    );
    assert!(generation.is_current());
    assert!(
        state
            .sessions()
            .unwrap()
            .plugin_library_progressive(false, |_| false)
            .await
            .is_ok(),
        "a failed switch reopens the selected profile"
    );
    drop(terminal);
    close(&state).await;
}

#[tokio::test]
async fn reserved_terminal_and_target_owner_failure_preserve_current_profile() {
    let temp = tempfile::tempdir().unwrap();
    let root = DataRoot::at(temp.path().join("data"));
    let state = state(&root);
    let profiles = Profiles::new(root);
    let first = profiles.open_active().unwrap();
    let next = profiles.create("Next").unwrap();
    let ticket = state.terminals.reserve("main").unwrap();
    assert!(
        activate(&state, next.clone(), sessions::testing::outside)
            .await
            .is_err()
    );
    assert!(!next.db_path().exists());
    drop(ticket);
    let owner = ProfileOwner::acquire(&next).unwrap();
    assert!(
        activate(&state, next, sessions::testing::outside)
            .await
            .is_err()
    );
    assert_eq!(profiles.open_active().unwrap().meta.id, first.meta.id);
    assert_eq!(state.app_info().profile.unwrap().id, first.meta.id);
    assert!(!state.app_info().profile_switching);
    add_chat(&state, temp.path()).await;
    close(&state).await;
    drop(owner);
}

/// What a Ghost account left in the data folder: a profile folder whose `profile.json` says
/// `account`, with an encrypted database. Returns its id and every file with its bytes.
fn account_leftover(root: &DataRoot) -> (String, Vec<(std::path::PathBuf, Vec<u8>)>) {
    let id = ghost_core::ids::new_id();
    let dir = root.profile_dir(&id).unwrap();
    std::fs::create_dir_all(dir.join("attachments")).unwrap();
    let files = [
        (
            "profile.json",
            format!(
                r#"{{"id":"{id}","kind":"account","display_name":"me@example.com","created_at":1}}"#
            )
            .into_bytes(),
        ),
        ("aura.encrypted.db", b"ciphertext, not SQLite".to_vec()),
        ("aura.encrypted.db.encryption", b"{\"version\":1}".to_vec()),
        ("attachments/blob", b"encrypted attachment".to_vec()),
    ];
    let written = files
        .into_iter()
        .map(|(name, bytes)| {
            let path = dir.join(name);
            std::fs::write(&path, &bytes).unwrap();
            (path, bytes)
        })
        .collect();
    (id, written)
}

/// Ghost accounts were removed. A data folder whose last-used profile was an account's starts
/// in Local (created here, since the account was the only profile), ready for chats; the
/// account's folder is not offered, cannot be switched to, and is still there byte for byte.
#[tokio::test]
async fn a_leftover_account_profile_starts_local_and_is_neither_listed_nor_touched() {
    let temp = tempfile::tempdir().unwrap();
    let root = DataRoot::at(temp.path().join("data"));
    root.ensure().unwrap();
    let (account, files) = account_leftover(&root);
    std::fs::write(
        root.state_file(),
        format!(r#"{{"active_profile":"{account}"}}"#),
    )
    .unwrap();

    let state = state(&root);
    let info = state.app_info();
    assert!(info.fatal.is_none(), "{:?}", info.fatal);
    assert_eq!(info.profile.as_ref().unwrap().id, "local");
    assert!(state.sessions().is_ok(), "chats open in Local");
    add_chat(&state, temp.path()).await;

    let profiles = Profiles::new(root.clone());
    let listed: Vec<String> = profiles
        .list()
        .unwrap()
        .into_iter()
        .map(|meta| meta.id)
        .collect();
    assert_eq!(listed, ["local"]);
    assert!(resolve(&state, account.clone()).await.is_err());
    assert_eq!(state.app_info().profile.unwrap().id, "local");
    assert_eq!(profiles.saved_active().as_deref(), Some("local"));

    close(&state).await;
    for (path, bytes) in &files {
        assert_eq!(&std::fs::read(path).unwrap(), bytes, "{}", path.display());
    }
    let dir = root.profile_dir(&account).unwrap();
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "attachments",
            "aura.encrypted.db",
            "aura.encrypted.db.encryption",
            "profile.json"
        ],
        "nothing added, moved or removed"
    );
}
