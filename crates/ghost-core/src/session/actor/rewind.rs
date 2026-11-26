//! A sent message's Rewind through the agent's own file checkpoints (Claude's `rewind_files`).
//! Ghost's checkpoints come first where a turn has one; this serves turns without one.
use super::*;
use ghost_drivers::driver::FileRewindResult;
use ghost_protocol::changes::NativeRewind;
use std::path::Path;

type Reply = oneshot::Sender<Result<NativeRewind, SessionError>>;

pub(super) struct Completion {
    generation: u64,
    reply: Reply,
    result: Result<
        Result<Result<FileRewindResult, DriverError>, oneshot::error::RecvError>,
        tokio::time::error::Elapsed,
    >,
}

/// Claude names files by absolute path. Inside the chat's folder they read as the rest of Changes
/// does: relative, with forward slashes. Windows compares the folder without regard to case.
fn relative(folder: &Path, file: String) -> String {
    let parts: Vec<_> = Path::new(&file).components().collect();
    let base: Vec<_> = folder.components().collect();
    let same = |a: &std::path::Component<'_>, b: &std::path::Component<'_>| {
        if cfg!(windows) {
            a.as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
        } else {
            a == b
        }
    };
    if parts.len() > base.len() && base.iter().zip(&parts).all(|(a, b)| same(a, b)) {
        parts[base.len()..]
            .iter()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    } else {
        file
    }
}

fn count(value: Option<u64>) -> Option<u32> {
    value.map(|value| u32::try_from(value).unwrap_or(u32::MAX))
}

impl ChatActor {
    /// Only an idle session whose agent advertised rewind takes one; the agent itself refuses a
    /// message it holds no checkpoint for.
    pub(super) fn rewind_files(&mut self, turn_id: String, dry_run: bool, reply: Reply) {
        if !self.capabilities.as_ref().is_some_and(|caps| caps.rewind) {
            let _ = reply.send(Err(SessionError::OptionInvalid(
                "This session cannot restore files from the agent's own checkpoints.".into(),
            )));
            return;
        }
        if !self.controls_idle() {
            let _ = reply.send(Err(SessionError::OptionInvalid(
                "Wait until the agent is idle, clear queued messages and answer pending requests before restoring files.".into(),
            )));
            return;
        }
        let (answer, receiver) = oneshot::channel();
        let generation = self.generation;
        self.rewind_pending = true;
        self.send_command(DriverCommand::RewindFiles {
            user_message_id: turn_id,
            dry_run,
            reply: answer,
        });
        self.in_flight.push(
            async move {
                Done::Rewind(Box::new(Completion {
                    generation,
                    reply,
                    result: tokio::time::timeout(OPTION_REPLY_TIMEOUT, receiver).await,
                }))
            }
            .boxed(),
        );
    }

    pub(super) fn rewind_done(&mut self, completion: Completion) {
        let Completion {
            generation,
            reply,
            result,
        } = completion;
        if generation != self.generation {
            let _ = reply.send(Err(SessionError::SessionNotReady));
            return;
        }
        self.rewind_pending = false;
        let result = match result {
            Ok(Ok(Ok(result))) => Ok(NativeRewind {
                files: result
                    .files_changed
                    .into_iter()
                    .map(|file| relative(&self.chat.folder, file))
                    .collect(),
                insertions: count(result.insertions),
                deletions: count(result.deletions),
                skipped_links: count(result.skipped_links),
            }),
            Ok(Ok(Err(error))) => Err(SessionError::OptionInvalid(error.message)),
            _ => Err(SessionError::OptionInvalid(
                "The agent did not confirm the file restore. Check the files before continuing."
                    .into(),
            )),
        };
        let _ = reply.send(result);
        self.dispatch_next();
    }
}

#[cfg(test)]
mod tests {
    use super::relative;
    use std::path::Path;

    #[test]
    fn files_in_the_chat_folder_read_relative_and_others_stay_whole() {
        let folder = Path::new("/work/project");
        assert_eq!(
            relative(folder, "/work/project/src/main.rs".into()),
            "src/main.rs"
        );
        assert_eq!(
            relative(folder, "/work/project/hello.txt".into()),
            "hello.txt"
        );
        assert_eq!(
            relative(folder, "/work/other/hello.txt".into()),
            "/work/other/hello.txt"
        );
        assert_eq!(relative(folder, "/work/project".into()), "/work/project");
        assert_eq!(relative(folder, "src/main.rs".into()), "src/main.rs");
    }

    #[cfg(windows)]
    #[test]
    fn windows_folders_match_whatever_their_case_and_separators() {
        let folder = Path::new("C:/Users/me/Project");
        assert_eq!(
            relative(folder, r"c:\users\me\project\src\main.rs".into()),
            "src/main.rs"
        );
    }
}
