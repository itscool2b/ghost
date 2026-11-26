//! Bounded, explicitly requested diagnostics from this app's own protocol logs.
use super::{LogFactory, Redactor, is_plain_name};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::fs::{Dir, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom};

const MAX_BYTES: u64 = 1024 * 1024;
const MAX_ENTRIES: usize = 4096;

impl LogFactory {
    /// Reads at most the last MiB of the newest spawn's current log. Only complete JSON records
    /// are included, redacted again with the current environment's known secret literals.
    /// No renderer-supplied path, vendor configuration, credential file or network is accessed.
    pub async fn debug_bundle(&self, chat_id: &str) -> io::Result<String> {
        if !is_plain_name(chat_id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid chat ID.",
            ));
        }
        let factory = self.clone();
        let chat_id = chat_id.to_owned();
        self.rt
            .spawn_blocking(move || factory.bundle(&chat_id))
            .await
            .map_err(io::Error::other)?
    }

    fn bundle(&self, chat_id: &str) -> io::Result<String> {
        let mut output = format!(
            "Ghost debug bundle v1\nApp: {}\nPlatform: {} / {}\nScope: newest saved protocol log; last 1 MiB; complete records only.\nTokens are redacted. Messages, code and local paths may remain. Review before sharing.\n\n",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        // Pin each directory below the already-owned profile. Refuse linked log/chat directories.
        let profile_path = self
            .root
            .parent()
            .and_then(|logs| logs.parent())
            .ok_or_else(|| io::Error::other("Protocol log root is unavailable."))?;
        let profile = Dir::open_ambient_dir(profile_path, cap_std::ambient_authority())?;
        let directory = profile
            .open_dir_nofollow("logs")
            .and_then(|logs| logs.open_dir_nofollow("protocol"))
            .and_then(|logs| logs.open_dir_nofollow(chat_id));
        let directory = match directory {
            Ok(directory) => directory,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                output.push_str("No saved protocol log is available.\n");
                return Ok(output);
            }
            Err(error) => return Err(error),
        };
        let mut newest = None;
        for (index, entry) in directory.entries()?.enumerate() {
            if index >= MAX_ENTRIES {
                return Err(io::Error::other(
                    "Too many protocol logs to prepare a bounded bundle.",
                ));
            }
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(id) = name.strip_suffix(".jsonl") else {
                continue;
            };
            // Spawn IDs are UUIDs; rotated files and unrelated files are excluded.
            if uuid::Uuid::parse_str(id).is_ok()
                && newest.as_deref().is_none_or(|old: &str| name > old)
            {
                newest = Some(name.to_owned());
            }
        }
        let Some(name) = newest else {
            output.push_str("No saved protocol log is available.\n");
            return Ok(output);
        };
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No).nonblock(true);
        let mut file = directory.open_with(&name, &options)?.into_std();
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(io::Error::other("The protocol log is not a regular file."));
        }
        let offset = metadata.len().saturating_sub(MAX_BYTES);
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = Vec::new();
        file.take(MAX_BYTES).read_to_end(&mut bytes)?;
        output.push_str(&redacted_records(&bytes, offset > 0, &self.redactor));
        Ok(output)
    }
}

fn redacted_records(bytes: &[u8], cut_start: bool, redactor: &Redactor) -> String {
    let mut output = String::new();
    let mut omitted = 0;
    for (index, line) in bytes.split_inclusive(|byte| *byte == b'\n').enumerate() {
        if (index == 0 && cut_start) || !line.ends_with(b"\n") {
            omitted += 1;
            continue;
        }
        let Ok(mut value) = serde_json::from_slice(line) else {
            omitted += 1;
            continue;
        };
        redactor.redact_value(&mut value);
        let record = serde_json::to_string(&value).unwrap_or_default();
        if output.len() + record.len() + 1 > MAX_BYTES as usize {
            omitted += 1;
            continue;
        }
        output.push_str(&record);
        output.push('\n');
    }
    if cut_start {
        output.push_str("[Earlier log bytes omitted.]\n");
    }
    if omitted > 0 {
        output.push_str(&format!(
            "[{omitted} partial, invalid or over-budget records omitted.]\n"
        ));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_complete_redacted_records_leave_the_log() {
        let redactor = Redactor::standard();
        redactor.set_secrets(vec!["owned-secret-value-123".into()]);
        let result = redacted_records(
            br#"{"password":"hidden","text":"owned-secret-value-123","inputTokens":42}
not JSON
{"token":"partial"#,
            false,
            &redactor,
        );
        assert!(!result.contains("hidden"));
        assert!(!result.contains("owned-secret"));
        assert!(!result.contains("partial\""));
        assert!(result.contains("\"inputTokens\":42"));
        assert!(result.contains("2 partial, invalid"));
        assert!(redacted_records(b"{}\n{}\n", true, &redactor).starts_with("{}\n[Earlier"));
    }

    #[tokio::test]
    async fn export_is_chat_scoped_bounded_and_refuses_linked_logs() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("logs/protocol");
        let dir = root.join("chat");
        std::fs::create_dir_all(&dir).unwrap();
        let name = format!("{}.jsonl", uuid::Uuid::now_v7());
        std::fs::write(dir.join(&name), b"{\"safe\":true}\n").unwrap();
        std::fs::write(dir.join("vendor.jsonl"), b"DO NOT READ").unwrap();
        let factory = LogFactory::new(
            root,
            std::sync::Arc::new(Redactor::standard()),
            tokio::runtime::Handle::current(),
        );
        let result = factory.debug_bundle("chat").await.unwrap();
        assert!(result.contains("\"safe\":true"));
        assert!(!result.contains("DO NOT READ"));
        let mut large = b"{\"old\":true}\n".to_vec();
        large.extend(vec![b' '; MAX_BYTES as usize]);
        large.extend_from_slice(b"\n{\"newest\":true}\n");
        std::fs::write(dir.join(&name), large).unwrap();
        let bounded = factory.debug_bundle("chat").await.unwrap();
        assert!(!bounded.contains("\"old\""));
        assert!(bounded.contains("\"newest\":true"));
        assert!(bounded.contains("Earlier log bytes omitted"));
        assert!(bounded.len() < MAX_BYTES as usize + 1024);
        assert!(factory.debug_bundle("../outside").await.is_err());
        assert!(
            factory
                .debug_bundle("missing")
                .await
                .unwrap()
                .contains("No saved")
        );
        #[cfg(unix)]
        {
            std::fs::remove_file(dir.join(&name)).unwrap();
            std::os::unix::fs::symlink(dir.join("vendor.jsonl"), dir.join(&name)).unwrap();
            assert!(factory.debug_bundle("chat").await.is_err());
        }
    }
}
