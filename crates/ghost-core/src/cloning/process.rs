//! Runs the reviewed `git clone` and settles its folder: kept when Git succeeds, removed when the
//! clone is canceled or fails.
use super::*;
use ghost_process::{LaunchSpec, ProcessCwd, ShutdownPlan, SpawnSpec, Spawner};
use std::ffi::OsString;
use tokio::io::{AsyncRead, AsyncReadExt};

/// A clone that has not finished in two hours is stopped; Cancel is the usual way out.
const LIMIT: Duration = Duration::from_secs(2 * 60 * 60);

pub(crate) async fn execute(prepared: Prepared, job: Arc<Job>, spawner: Arc<Spawner>) {
    execute_within(prepared, job, spawner, LIMIT).await;
}

pub(super) async fn execute_within(
    prepared: Prepared,
    job: Arc<Job>,
    spawner: Arc<Spawner>,
    limit: Duration,
) {
    if *job.canceled.borrow() {
        job.finish(CloneState::Canceled, "Clone canceled before it started.");
        return;
    }
    let destination = match tokio::task::spawn_blocking(move || files::begin(prepared)).await {
        Ok(Ok(destination)) => destination,
        Ok(Err(error)) => {
            job.finish(CloneState::Failed, &error.to_string());
            return;
        }
        Err(_) => {
            job.finish(CloneState::Failed, "The new folder could not be created.");
            return;
        }
    };
    if let Some(journal) = &job.journal
        && let Err(error) = journal.created(&destination)
    {
        // The newly created folder may be the only evidence of this interrupted attempt.
        // Do not run Git or remove the folder after losing durable lifecycle recording.
        job.update(|record| {
            record.state = CloneState::Interrupted;
            record.detail = error.to_string();
        });
        return;
    }
    let outcome = run(&destination, &job, &spawner, limit).await;
    let canceled = matches!(outcome, Outcome::Canceled);
    let (state, mut detail, keep) = match outcome {
        Outcome::Cloned => {
            if let Some(journal) = &job.journal
                && let Err(error) = journal.git_finished(&destination)
            {
                job.update(|record| {
                    record.state = CloneState::Interrupted;
                    record.detail = error.to_string();
                });
                return;
            }
            let checked = tokio::task::spawn_blocking(move || {
                let result = files::verify(&destination);
                (destination, result)
            })
            .await;
            match checked {
                Ok((destination, Ok(()))) => {
                    let repository = destination.prepared.view.repository.clone();
                    job.update(|record| {
                        if let Some(progress) = &mut record.progress {
                            progress.percent = 100;
                        }
                    });
                    job.finish(CloneState::Ready, &format!("Cloned {repository}."));
                    return;
                }
                // Git said it succeeded but the folder is not what it made: never remove it.
                Ok((_, Err(error))) => {
                    job.finish(
                        CloneState::Failed,
                        &format!("{error} The folder was kept as it is."),
                    );
                    return;
                }
                Err(_) => {
                    job.finish(
                        CloneState::Failed,
                        "The cloned folder could not be checked.",
                    );
                    return;
                }
            }
        }
        Outcome::Canceled => (
            CloneState::Canceled,
            "Clone canceled.".to_owned(),
            destination,
        ),
        Outcome::Failed(reason) => (CloneState::Failed, reason, destination),
        Outcome::NotStarted(reason) => (CloneState::Failed, reason, destination),
    };
    let path = keep.prepared.view.destination.clone();
    let removed = tokio::task::spawn_blocking(move || files::remove(keep))
        .await
        .unwrap_or(false);
    if removed {
        detail.push_str(" The unfinished folder was removed.");
    } else {
        detail.push_str(&format!(
            " The unfinished folder changed or could not be removed, so it was kept at {path}."
        ));
    }
    job.update(|record| {
        record.removed = removed;
        if canceled {
            record.progress = None;
        }
    });
    job.finish(state, &detail);
}

enum Outcome {
    Cloned,
    Canceled,
    Failed(String),
    NotStarted(String),
}

async fn run(
    destination: &files::Destination,
    job: &Arc<Job>,
    spawner: &Spawner,
    limit: Duration,
) -> Outcome {
    if *job.canceled.borrow() {
        return Outcome::Canceled;
    }
    let spec = spec(&destination.prepared);
    let raw = match tokio::time::timeout(Duration::from_secs(15), spawner.spawn_raw(spec)).await {
        Ok(Ok(raw)) => raw,
        Ok(Err(_)) => {
            return Outcome::NotStarted(
                "Git could not start. Check that Git is installed and the folder can be written."
                    .into(),
            );
        }
        Err(_) => return Outcome::NotStarted("Git did not start within fifteen seconds.".into()),
    };
    raw.stdin.close();
    job.update(|record| record.detail = "Cloning.".into());
    let stdout = tokio::spawn(discard(raw.stdout));
    let stderr = tokio::spawn(follow(raw.stderr, Arc::clone(job)));
    let mut canceled = job.canceled.subscribe();
    enum Stop {
        Exited(Option<i32>),
        Canceled,
        Timeout,
    }
    let stopped = tokio::select! {
        biased;
        _ = canceled.wait_for(|value| *value) => Stop::Canceled,
        exit = raw.control.exited() => Stop::Exited(exit.code),
        _ = tokio::time::sleep(limit) => Stop::Timeout,
    };
    // Always stop the whole owned tree, including helpers left after Git exits. The folder is
    // not touched until the stop is confirmed.
    loop {
        let report = raw
            .control
            .retry_unconfirmed_shutdown(ShutdownPlan::FORCE)
            .await;
        if report.exit.is_some() && report.leftovers.is_empty() {
            break;
        }
        job.finish(
            CloneState::CleanupPending,
            "Waiting for Git to stop before removing the unfinished folder.",
        );
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    let mut stderr = stderr;
    let output = tokio::time::timeout(Duration::from_secs(2), &mut stderr)
        .await
        .ok()
        .and_then(|result| result.ok())
        .unwrap_or_default();
    stderr.abort();
    stdout.abort();
    match stopped {
        Stop::Exited(Some(0)) => Outcome::Cloned,
        Stop::Exited(_) => Outcome::Failed(failure(&output, &destination.prepared.view.parent)),
        Stop::Canceled => Outcome::Canceled,
        Stop::Timeout => Outcome::Failed("The clone was stopped after two hours.".into()),
    }
}

fn spec(prepared: &Prepared) -> SpawnSpec {
    // Git starts in the new, empty folder and may not look above it, so a repository that
    // happens to contain the chosen parent cannot lend its config (an SSH command, a credential
    // helper, fsmonitor) to the clone. The person's global config still applies.
    let folder = prepared.parent.join(&prepared.name);
    let ceiling = plain_path(&prepared.parent).into_os_string();
    SpawnSpec {
        launch: LaunchSpec {
            label: "git clone".into(),
            program: prepared.program.clone(),
            args: prepared.arguments.iter().map(OsString::from).collect(),
            cwd: ProcessCwd::Fixed(folder.clone()),
            // No terminal is attached, so Git must not wait for one. Credential helpers and
            // SSH agents the person set up still answer, in their own windows if they have any.
            set_env: vec![
                ("GIT_TERMINAL_PROMPT".into(), "0".into()),
                ("GIT_CEILING_DIRECTORIES".into(), ceiling),
            ],
            daemons: vec![],
            startup_timeout: Duration::from_secs(15),
        },
        folder,
        env: Arc::clone(&prepared.env),
        max_line_bytes: ghost_process::MAX_LINE_BYTES,
    }
}

async fn discard(mut reader: impl AsyncRead + Unpin) {
    let mut bytes = [0; 8192];
    while reader.read(&mut bytes).await.is_ok_and(|count| count > 0) {}
}

/// Follows Git's stderr into the record and returns the kept lines for the failure reason.
async fn follow(mut reader: impl AsyncRead + Unpin, job: Arc<Job>) -> Vec<String> {
    let mut parser = progress::Parser::default();
    let mut bytes = [0; 8192];
    loop {
        let count = match reader.read(&mut bytes).await {
            Ok(0) | Err(_) => break,
            Ok(count) => count,
        };
        if parser.feed(&bytes[..count]) {
            publish(&job, &parser);
        }
    }
    if parser.finish() {
        publish(&job, &parser);
    }
    parser.output
}

fn publish(job: &Job, parser: &progress::Parser) {
    job.update(|record| {
        record.progress.clone_from(&parser.progress);
        record.output.clone_from(&parser.output);
    });
}

/// One plain sentence for why Git stopped. Git's own lines are shown separately, never mixed in.
fn failure(lines: &[String], parent: &str) -> String {
    let text = lines.join("\n").to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|needle| text.contains(needle));
    if has(&["host key verification failed"]) {
        "SSH does not trust this server yet. Connect to it once in your terminal to confirm its key, then try again.".into()
    } else if has(&[
        "authentication failed",
        "could not read username",
        "could not read password",
        "permission denied (publickey",
        "terminal prompts disabled",
        "invalid username or password",
        "access denied",
    ]) {
        "Git could not sign in. Set up your credential helper or SSH key in your terminal, then try again.".into()
    } else if has(&[
        "repository not found",
        "not found",
        "does not appear to be a git repository",
        "does not exist",
    ]) {
        "Git could not find this repository. Check the address and that your account can see it."
            .into()
    } else if has(&["no space left", "disk full", "not enough space"]) {
        "The disk is full. Free some space, then try again.".into()
    } else if has(&["filename too long", "path too long"]) {
        "A path in this repository is too long for this folder. Choose a folder nearer the drive, or turn on long paths in Git.".into()
    } else if has(&[
        "could not resolve",
        "failed to connect",
        "connection timed out",
        "connection refused",
        "connection reset",
        "network is unreachable",
        "operation timed out",
        "unable to access",
        "ssl",
        "early eof",
    ]) {
        "Git could not reach the server. Check the connection and the address, then try again."
            .into()
    } else if has(&["permission denied", "unable to create", "could not create"]) {
        format!("Git could not write to {parent}. Choose a folder you can write to.")
    } else {
        "Git stopped before the clone finished.".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reasons_are_plain_sentences_that_never_repeat_gits_output() {
        for (line, reason) in [
            (
                "fatal: Authentication failed for 'https://x' secret",
                "sign in",
            ),
            (
                "remote: Repository not found. private",
                "find this repository",
            ),
            (
                "Host key verification failed. private",
                "SSH does not trust",
            ),
            (
                "fatal: unable to access 'https://x/': Could not resolve host: x private",
                "reach",
            ),
            (
                "error: unable to write file: No space left on device private",
                "disk is full",
            ),
            (
                "fatal: cannot mkdir: Permission denied private",
                "could not write to C:\\code",
            ),
            ("something unexpected private", "stopped before"),
        ] {
            let message = failure(&[line.into()], "C:\\code");
            assert!(message.contains(reason), "{line}: {message}");
            assert!(!message.contains("private") && !message.contains("secret"));
        }
    }
}
