//! Whether the person, or their organisation, has told Claude Code to ask before something: an
//! `ask` rule in one of the places Claude takes its permission rules from.
//!
//! Claude asks about a command made of several by saying only that its parts were looked at
//! (`subcommandResults`), not why any one of them asks. So a planning turn on Full access cannot
//! tell "Claude could not prove this read-only" from "one part matched an ask rule". Such a
//! request is answered for the person only while no ask rule can exist (`Session::plans_unasked`),
//! and this module errs toward "one can": a place that cannot be found, read or understood counts
//! as holding one.
//!
//! One place is out of reach: settings an organisation sends from Anthropic's servers are looked
//! for only in the file Claude is known to keep them in. An organisation that manages Claude that
//! way normally turns Full access off as well, and without Full access granted nothing is
//! answered at all.

use std::{
    ffi::OsString,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde_json::Value;

use crate::driver::SessionSetup;

/// Where Windows keeps Claude Code's managed policy, for the machine and for the user.
const POLICY_KEY: &str = r"SOFTWARE\Policies\ClaudeCode";

/// The settings files Claude Code takes permission rules from for a chat in `folder`: the
/// person's own and what their organisation sent (`config_dir`), the project's two in the folder
/// and in each folder above it and in each folder added to the chat, and the ones an
/// organisation manages on this computer.
pub(super) fn settings_files(config_dir: &Path, folder: &Path, added: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = vec![
        config_dir.join("settings.json"),
        config_dir.join("remote-settings.json"),
    ];
    for dir in folder.ancestors().chain(added.iter().map(PathBuf::as_path)) {
        files.push(dir.join(".claude").join("settings.json"));
        files.push(dir.join(".claude").join("settings.local.json"));
    }
    for managed in [
        r"C:\Program Files\ClaudeCode\managed-settings.json",
        r"C:\ProgramData\ClaudeCode\managed-settings.json",
        "/Library/Application Support/ClaudeCode/managed-settings.json",
        "/Library/Managed Preferences/com.anthropic.claudecode.plist",
        "/etc/claude-code/managed-settings.json",
    ] {
        files.push(PathBuf::from(managed));
    }
    files
}

/// Whether any of `files` holds an ask rule. A file that is there but cannot be read or
/// understood counts as holding one: it cannot be ruled out. (A managed preferences list is not
/// JSON, so its presence alone counts.)
fn a_file_asks(files: &[PathBuf]) -> bool {
    files.iter().any(|file| match std::fs::read(file) {
        Err(error) => !matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory),
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Err(_) => true,
            Ok(settings) => match &settings["permissions"]["ask"] {
                Value::Null => false,
                Value::Array(rules) => !rules.is_empty(),
                _ => true,
            },
        },
    })
}

/// Whether an ask rule may exist for a chat whose rules come from `sources`. With no sources (the
/// person's settings folder could not be found) the answer is yes; so it is where this computer
/// has a managed policy for Claude Code in its registry, whatever that policy says.
pub(super) fn asks_by_rule(sources: Option<&[PathBuf]>) -> bool {
    match sources {
        None => true,
        Some(files) => ghost_process::env::policy_key_exists(POLICY_KEY) || a_file_asks(files),
    }
}

/// The places an ask rule could be written for this chat, or `None` when the person's settings
/// folder cannot be worked out, so they cannot be looked at.
pub(super) fn sources(setup: &SessionSetup) -> Option<Vec<PathBuf>> {
    Some(settings_files(
        &config_dir(setup)?,
        &setup.folder,
        &setup.add_dirs,
    ))
}

/// Where this chat's Claude keeps the person's settings: the folder its launch names
/// (`CLAUDE_CONFIG_DIR`, which a chat on an added account has), else the one the agent's
/// environment names, else `.claude` in the home folder. A folder that is named but not in full
/// is not guessed at.
fn config_dir(setup: &SessionSetup) -> Option<PathBuf> {
    let named = |name: &str| -> Option<OsString> {
        setup
            .launch
            .set_env
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
            .or_else(|| {
                setup
                    .env
                    .vars
                    .get(&ghost_process::env::EnvKey::new(name))
                    .cloned()
            })
            .filter(|value| !value.is_empty())
    };
    named("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            named("USERPROFILE")
                .or_else(|| named("HOME"))
                .map(|home| PathBuf::from(home).join(".claude"))
        })
        .filter(|dir| dir.is_absolute())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ask_rule_in_any_settings_file_is_found_and_an_unreadable_file_counts_as_one() {
        let root = tempfile::tempdir().expect("folder");
        let home = root.path().join("home").join(".claude");
        let project = root.path().join("work").join("app");
        let added = root.path().join("shared");
        std::fs::create_dir_all(&home).expect("home");
        std::fs::create_dir_all(project.join(".claude")).expect("project");
        std::fs::create_dir_all(added.join(".claude")).expect("added");
        let files = settings_files(&home, &project, std::slice::from_ref(&added));
        assert_eq!(files[0], home.join("settings.json"));
        assert!(files.contains(&home.join("remote-settings.json")));
        assert!(files.contains(&project.join(".claude").join("settings.local.json")));
        // The folders above the project are looked in too, and each folder added to the chat.
        assert!(
            files.contains(
                &root
                    .path()
                    .join("work")
                    .join(".claude")
                    .join("settings.json")
            )
        );
        assert!(files.contains(&added.join(".claude").join("settings.json")));
        let here: Vec<PathBuf> = files
            .iter()
            .filter(|file| file.starts_with(root.path()))
            .cloned()
            .collect();

        // No file at all, then files without an ask rule.
        assert!(!a_file_asks(&here));
        std::fs::write(
            home.join("settings.json"),
            r#"{"permissions":{"allow":["Bash(git status:*)"],"ask":[]}}"#,
        )
        .expect("write");
        std::fs::write(project.join(".claude").join("settings.json"), "{}").expect("write");
        assert!(!a_file_asks(&here));

        // One ask rule anywhere is enough: the project's, an added folder's, the organisation's.
        for file in [
            project.join(".claude").join("settings.local.json"),
            added.join(".claude").join("settings.json"),
            home.join("remote-settings.json"),
        ] {
            std::fs::write(&file, r#"{"permissions":{"ask":["Bash(git push:*)"]}}"#)
                .expect("write");
            assert!(a_file_asks(&here), "{}", file.display());
            std::fs::remove_file(&file).expect("remove");
            assert!(!a_file_asks(&here));
        }

        // A file that cannot be understood, or whose ask is not a list, cannot be ruled out.
        std::fs::write(home.join("settings.json"), "{ not json").expect("write");
        assert!(a_file_asks(&here));
        std::fs::write(
            home.join("settings.json"),
            r#"{"permissions":{"ask":"Bash"}}"#,
        )
        .expect("write");
        assert!(a_file_asks(&here));
    }

    #[test]
    fn where_the_settings_cannot_be_looked_at_a_rule_is_assumed() {
        assert!(asks_by_rule(None));
    }
}
