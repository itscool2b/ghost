use super::super::git;
use super::super::tests::{Rig, rig};
use super::*;

async fn output(rig: &Rig, folder: &Path, values: &[&str]) -> String {
    String::from_utf8(
        rig.git
            .run(folder, git::args(values), 64 * 1024)
            .await
            .expect("fixture git"),
    )
    .expect("utf-8")
}

fn input(message: &str, paths: &[&str]) -> CommitInput {
    CommitInput {
        message: message.into(),
        paths: paths.iter().map(|path| (*path).to_owned()).collect(),
    }
}

async fn commit(rig: &Rig, folder: &Path, paths: &[&str]) -> String {
    let preview = rig
        .service
        .commit_preview(folder, input("Fixture change", paths))
        .await
        .expect("commit preview");
    rig.service
        .commit_apply(folder, &preview.token)
        .await
        .expect("commit")
        .commit
}

#[test]
fn messages_are_kept_as_git_keeps_them() {
    assert_eq!(
        clean_message("\n\nChange the file  \n\n\n\nand add one\t\n\n").unwrap(),
        "Change the file\n\nand add one\n"
    );
    assert!(clean_message(" \n\t\n").is_err());
    assert!(clean_message("nul\0byte").is_err());
}

#[test]
fn shown_remote_addresses_leave_out_credentials() {
    // Built at run time, so no literal address with a user and password sits in the source.
    let userinfo = ["someone", "not-a-real-password"].join(":");
    assert_eq!(
        without_credentials(&format!("https://{userinfo}@github.com/owner/repo.git")),
        "https://github.com/owner/repo.git"
    );
    assert_eq!(
        without_credentials("ssh://git@host.example:2222/repo"),
        "ssh://host.example:2222/repo"
    );
    assert_eq!(
        without_credentials("git@github.com:owner/repo.git"),
        "git@github.com:owner/repo.git"
    );
    assert_eq!(without_credentials("https://host"), "https://host");
}

#[tokio::test]
async fn a_commit_takes_exactly_the_chosen_files_and_leaves_staged_work_alone() {
    let rig = rig().await;
    std::fs::write(rig.work.join("file.txt"), "changed\n").unwrap();
    // A name with a glob character is taken literally.
    std::fs::write(rig.work.join("new [1].txt"), "new\n").unwrap();
    std::fs::write(rig.work.join("kept.txt"), "staged\n").unwrap();
    std::fs::write(rig.work.join("later.txt"), "not chosen\n").unwrap();
    output(&rig, &rig.work, &["add", "--", "kept.txt"]).await;
    let preview = rig
        .service
        .commit_preview(
            &rig.work,
            input(
                "Change the file\n\n\nand add one  \n",
                &["file.txt", "new [1].txt"],
            ),
        )
        .await
        .expect("preview");
    assert_eq!(preview.branch, "main");
    assert_eq!(preview.author, "Fixture <fixture@localhost>");
    assert_eq!(preview.message, "Change the file\n\nand add one\n");
    assert!(!preview.signed);
    assert_eq!(preview.files.len(), 2);
    assert!(
        preview.diff_stat.contains("file.txt") && preview.diff_stat.contains("new [1].txt"),
        "{}",
        preview.diff_stat
    );
    assert!(!preview.diff_stat.contains("kept.txt"));
    // The review staged nothing.
    assert_eq!(
        output(&rig, &rig.work, &["status", "--porcelain=v1"]).await,
        " M file.txt\nA  kept.txt\n?? later.txt\n?? \"new [1].txt\"\n"
    );
    let result = rig
        .service
        .commit_apply(&rig.work, &preview.token)
        .await
        .expect("commit");
    assert!(result.warning.is_none(), "{:?}", result.warning);
    assert_eq!(
        output(&rig, &rig.work, &["rev-parse", "HEAD"]).await.trim(),
        result.commit
    );
    assert_eq!(
        output(&rig, &rig.work, &["show", "HEAD:file.txt"]).await,
        "changed\n"
    );
    assert_eq!(
        output(&rig, &rig.work, &["show", "HEAD:new [1].txt"]).await,
        "new\n"
    );
    assert_eq!(
        output(&rig, &rig.work, &["log", "-1", "--format=%B"]).await,
        "Change the file\n\nand add one\n\n"
    );
    // The staged file stays staged and the other stays untracked.
    assert_eq!(
        output(&rig, &rig.work, &["status", "--porcelain=v1"]).await,
        "A  kept.txt\n?? later.txt\n"
    );
    // A review is used once.
    assert!(
        rig.service
            .commit_apply(&rig.work, &preview.token)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_commit_is_refused_once_its_files_or_branch_moved() {
    let rig = rig().await;
    let before = output(&rig, &rig.work, &["rev-parse", "HEAD"]).await;
    std::fs::write(rig.work.join("file.txt"), "reviewed\n").unwrap();
    let preview = rig
        .service
        .commit_preview(&rig.work, input("Reviewed", &["file.txt"]))
        .await
        .expect("preview");
    std::fs::write(rig.work.join("file.txt"), "edited after the review\n").unwrap();
    let error = rig
        .service
        .commit_apply(&rig.work, &preview.token)
        .await
        .expect_err("stale files");
    assert!(
        error.to_string().contains("changed since the review"),
        "{error}"
    );
    let preview = rig
        .service
        .commit_preview(&rig.work, input("Reviewed", &["file.txt"]))
        .await
        .expect("preview");
    std::fs::write(rig.work.join("other.txt"), "elsewhere\n").unwrap();
    commit(&rig, &rig.work, &["other.txt"]).await;
    let error = rig
        .service
        .commit_apply(&rig.work, &preview.token)
        .await
        .expect_err("moved branch");
    assert!(error.to_string().contains("branch moved"), "{error}");
    assert_ne!(
        output(&rig, &rig.work, &["rev-parse", "HEAD"]).await,
        before
    );
    assert_eq!(
        output(&rig, &rig.work, &["show", "HEAD:file.txt"]).await,
        "initial\n"
    );
}

#[tokio::test]
async fn a_new_repository_takes_its_first_commit() {
    let rig = rig().await;
    let fresh = rig.work.parent().unwrap().join("fresh");
    std::fs::create_dir(&fresh).unwrap();
    output(&rig, &fresh, &["init", "-b", "trunk"]).await;
    for (key, value) in [
        ("user.name", "Fixture"),
        ("user.email", "fixture@localhost"),
    ] {
        output(&rig, &fresh, &["config", key, value]).await;
    }
    std::fs::write(fresh.join("readme.txt"), "hello\n").unwrap();
    let preview = rig
        .service
        .commit_preview(&fresh, input("First", &["readme.txt"]))
        .await
        .expect("preview");
    assert_eq!(preview.parent, None);
    assert_eq!(preview.branch, "trunk");
    rig.service
        .commit_apply(&fresh, &preview.token)
        .await
        .expect("first commit");
    assert_eq!(
        output(&rig, &fresh, &["rev-list", "--count", "HEAD"])
            .await
            .trim(),
        "1"
    );
    assert_eq!(
        output(&rig, &fresh, &["status", "--porcelain=v1"]).await,
        ""
    );
}

#[tokio::test]
async fn a_push_only_fast_forwards_and_sets_the_upstream_it_creates() {
    let rig = rig().await;
    let parent = rig.work.parent().unwrap().to_owned();
    let remote = parent.join("remote.git");
    output(
        &rig,
        &parent,
        &["init", "--bare", "-b", "main", remote.to_str().unwrap()],
    )
    .await;
    output(
        &rig,
        &rig.work,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    )
    .await;
    std::fs::write(rig.work.join("file.txt"), "pushed\n").unwrap();
    let first = commit(&rig, &rig.work, &["file.txt"]).await;
    let preview = rig
        .service
        .push_preview(&rig.work)
        .await
        .expect("push preview");
    assert_eq!(
        (preview.remote.as_str(), preview.target.as_str()),
        ("origin", "main")
    );
    assert!(preview.sets_upstream);
    assert_eq!(preview.remote_head, None);
    assert!(
        preview
            .warnings
            .iter()
            .any(|warning| warning.contains("folder on this computer")),
        "{:?}",
        preview.warnings
    );
    assert_eq!(preview.count, 2);
    assert_eq!(preview.commits.len(), 2);
    assert!(
        preview.commits[0].ends_with(" Fixture change"),
        "{:?}",
        preview.commits
    );
    let result = rig
        .service
        .push_apply(&rig.work, &preview.token)
        .await
        .expect("push");
    assert!(result.created);
    assert!(result.warning.is_none(), "{:?}", result.warning);
    assert!(
        output(&rig, &rig.work, &["ls-remote", "origin", "refs/heads/main"])
            .await
            .starts_with(&first)
    );
    assert_eq!(
        output(&rig, &rig.work, &["config", "branch.main.remote"])
            .await
            .trim(),
        "origin"
    );
    let error = rig
        .service
        .push_preview(&rig.work)
        .await
        .expect_err("nothing to push");
    assert!(error.to_string().contains("already has"), "{error}");
    // The next push fast-forwards the upstream.
    std::fs::write(rig.work.join("file.txt"), "pushed again\n").unwrap();
    commit(&rig, &rig.work, &["file.txt"]).await;
    let preview = rig.service.push_preview(&rig.work).await.expect("preview");
    assert!(!preview.sets_upstream);
    assert_eq!(preview.remote_head.as_deref(), Some(first.as_str()));
    assert_eq!(preview.count, 1);
    rig.service
        .push_apply(&rig.work, &preview.token)
        .await
        .expect("fast-forward");
    // Commits this folder has not seen on the remote stop a push before it is reviewed.
    let other = parent.join("other");
    output(
        &rig,
        &parent,
        &[
            "clone",
            "--quiet",
            remote.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    )
    .await;
    std::fs::write(other.join("theirs.txt"), "theirs\n").unwrap();
    output(&rig, &other, &["add", "--", "theirs.txt"]).await;
    output(
        &rig,
        &other,
        &[
            "-c",
            "user.name=Other",
            "-c",
            "user.email=other@localhost",
            "-c",
            "commit.gpgSign=false",
            "commit",
            "-q",
            "-m",
            "theirs",
        ],
    )
    .await;
    output(&rig, &other, &["push", "--quiet", "origin", "main"]).await;
    std::fs::write(rig.work.join("file.txt"), "diverged\n").unwrap();
    commit(&rig, &rig.work, &["file.txt"]).await;
    let error = rig
        .service
        .push_preview(&rig.work)
        .await
        .expect_err("unseen remote commits");
    assert!(error.to_string().contains("Fetch with Git"), "{error}");
}

#[tokio::test]
async fn programs_set_by_the_repository_itself_stop_signing_and_pushing() {
    let rig = rig().await;
    std::fs::write(rig.work.join("file.txt"), "signed?\n").unwrap();
    output(&rig, &rig.work, &["config", "commit.gpgSign", "true"]).await;
    // Through a file the repository's configuration includes, as an agent could arrange it.
    std::fs::write(
        rig.work.join("signing.inc"),
        "[gpg]\n\tprogram = not-a-signer\n",
    )
    .unwrap();
    let include = rig.work.join("signing.inc");
    output(
        &rig,
        &rig.work,
        &["config", "include.path", include.to_str().unwrap()],
    )
    .await;
    let error = rig
        .service
        .commit_preview(&rig.work, input("Signed", &["file.txt"]))
        .await
        .expect_err("a repository-chosen signing program");
    assert!(error.to_string().contains("gpg.program"), "{error}");
    output(&rig, &rig.work, &["config", "--unset", "include.path"]).await;
    output(&rig, &rig.work, &["config", "commit.gpgSign", "false"]).await;
    let parent = rig.work.parent().unwrap().to_owned();
    let remote = parent.join("remote.git");
    output(
        &rig,
        &parent,
        &["init", "--bare", "-b", "main", remote.to_str().unwrap()],
    )
    .await;
    output(
        &rig,
        &rig.work,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    )
    .await;
    output(&rig, &rig.work, &["config", "core.sshCommand", "not-ssh"]).await;
    let error = rig
        .service
        .push_preview(&rig.work)
        .await
        .expect_err("a repository-chosen SSH command");
    assert!(error.to_string().contains("core.sshcommand"), "{error}");
}

#[tokio::test]
async fn a_push_shows_every_address_and_is_refused_if_they_change() {
    let rig = rig().await;
    let parent = rig.work.parent().unwrap().to_owned();
    let [one, two, three] = ["one.git", "two.git", "three.git"].map(|name| parent.join(name));
    for remote in [&one, &two, &three] {
        output(
            &rig,
            &parent,
            &["init", "--bare", "-b", "main", remote.to_str().unwrap()],
        )
        .await;
    }
    output(
        &rig,
        &rig.work,
        &["remote", "add", "origin", one.to_str().unwrap()],
    )
    .await;
    for remote in [&one, &two] {
        output(
            &rig,
            &rig.work,
            &[
                "config",
                "--add",
                "remote.origin.pushurl",
                remote.to_str().unwrap(),
            ],
        )
        .await;
    }
    let preview = rig.service.push_preview(&rig.work).await.expect("preview");
    assert_eq!(preview.remote_urls.len(), 2, "{:?}", preview.remote_urls);
    output(
        &rig,
        &rig.work,
        &[
            "remote",
            "set-url",
            "--push",
            "--add",
            "origin",
            three.to_str().unwrap(),
        ],
    )
    .await;
    let error = rig
        .service
        .push_apply(&rig.work, &preview.token)
        .await
        .expect_err("a changed address");
    assert!(error.to_string().contains("address changed"), "{error}");
    assert!(
        output(
            &rig,
            &parent,
            &["--git-dir", three.to_str().unwrap(), "for-each-ref"]
        )
        .await
        .is_empty()
    );
}

#[test]
fn folder_addresses_and_remote_helpers_are_recognized() {
    for folder in [
        "/srv/git/repo.git",
        "../repo.git",
        "C:\\work\\repo.git",
        "C:/work/repo.git",
        "\\\\server\\share\\repo.git",
        "file:///srv/git/repo.git",
    ] {
        assert!(local_address(folder), "{folder}");
    }
    for remote in [
        "git@github.com:owner/repo.git",
        "https://github.com/owner/repo.git",
        "ssh://git@host.example/repo",
    ] {
        assert!(!local_address(remote), "{remote}");
    }
    assert_eq!(remote_helper("ext::sh -c true"), Some("ext"));
    assert_eq!(
        remote_helper("persistent-https::host/repo"),
        Some("persistent-https")
    );
    assert_eq!(remote_helper("https://host/a::b"), None);
    assert_eq!(remote_helper("git@github.com:owner/repo.git"), None);
}

#[tokio::test]
async fn a_repository_filter_stops_a_commit_and_a_helper_remote_stops_a_push() {
    let rig = rig().await;
    std::fs::write(rig.work.join("file.txt"), "filtered?\n").unwrap();
    output(
        &rig,
        &rig.work,
        &["config", "filter.fixture.clean", "not-a-filter"],
    )
    .await;
    let error = rig
        .service
        .commit_preview(&rig.work, input("Filtered", &["file.txt"]))
        .await
        .expect_err("a repository-chosen filter");
    assert!(
        error.to_string().contains("filter.fixture.clean"),
        "{error}"
    );
    output(
        &rig,
        &rig.work,
        &["config", "--unset", "filter.fixture.clean"],
    )
    .await;
    output(
        &rig,
        &rig.work,
        &["remote", "add", "origin", "ext::not-a-command"],
    )
    .await;
    let error = rig
        .service
        .push_preview(&rig.work)
        .await
        .expect_err("a remote helper");
    assert!(
        error.to_string().contains("remote helper (ext::)"),
        "{error}"
    );
}

#[tokio::test]
async fn remote_programs_are_pinned_to_the_persons_own_or_gits_defaults() {
    let rig = rig().await;
    // Even a setting that slipped past the check is outranked by the command line.
    output(
        &rig,
        &rig.work,
        &["config", "remote.origin.uploadpack", "not-upload-pack"],
    )
    .await;
    let pins: Vec<String> = remote_pins(&rig.git, &rig.work, Some("origin"))
        .await
        .expect("pins")
        .into_iter()
        .map(|pin| pin.to_string_lossy().into_owned())
        .collect();
    for expected in [
        "core.askPass=",
        "core.gitProxy=none",
        "remote.origin.receivepack=git-receive-pack",
        "protocol.ext.allow=never",
        "credential.helper=",
    ] {
        assert!(
            pins.iter().any(|pin| pin == expected),
            "{expected} missing from {pins:?}"
        );
    }
    assert!(pins.iter().any(|pin| pin.starts_with("core.sshCommand=")));
    assert!(
        pins.iter()
            .any(|pin| pin.starts_with("remote.origin.uploadpack=")
                && !pin.contains("not-upload-pack")),
        "{pins:?}"
    );
    // Every value follows its own -c.
    assert!(pins.chunks(2).all(|pair| pair[0] == "-c"));
}
