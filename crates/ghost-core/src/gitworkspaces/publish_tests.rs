use super::*;
use crate::gitworkspaces::tests::{Rig, commit_file, isolated, rig};
use std::sync::Mutex;

struct FixturePublisher(Mutex<State>);
struct State {
    base: String,
    head: Option<String>,
    created: bool,
    pushes: usize,
    bodies: Vec<Vec<u8>>,
    creates: usize,
}

impl Publisher for FixturePublisher {
    fn command<'a>(
        &'a self,
        _folder: &'a Path,
        args: Vec<OsString>,
        input: Option<Vec<u8>>,
    ) -> BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move {
            let args = args
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            let mut state = self.0.lock().expect("fixture");
            match (args[0].as_str(),args[1].as_str()) {
                ("repo","view")=>Ok(br#"{"nameWithOwner":"fixture/project","url":"https://github.com/fixture/project","defaultBranchRef":{"name":"main"}}"#.to_vec()),
                ("pr","list")=>Ok(serde_json::to_vec(&if state.created { serde_json::json!([{"url":"https://github.com/fixture/project/pull/17","baseRefName":"main","headRefOid":state.head,"isCrossRepository":false}]) } else {serde_json::json!([])}).expect("json")),
                ("pr","create")=>{
                    assert!(args.windows(2).any(|pair|pair==["--head","ghost/feature"]));
                    assert!(args.windows(2).any(|pair|pair==["--body-file","-"]));
                    assert!(!args.iter().any(|arg|arg=="--draft" || arg=="--fill" || arg=="--body"));
                    state.bodies.push(input.expect("body stdin")); state.created=true; state.creates+=1;
                    Ok(b"https://github.com/fixture/project/pull/17\n".to_vec())
                },
                _=>panic!("unexpected fixture command {args:?}"),
            }
        })
    }
    fn branch<'a>(
        &'a self,
        _git: &'a Git,
        _folder: &'a Path,
        _remote: &'a str,
        branch: &'a str,
    ) -> BoxFuture<'a, Result<Option<String>>> {
        Box::pin(async move {
            let state = self.0.lock().expect("fixture");
            Ok(if branch == "main" {
                Some(state.base.clone())
            } else {
                state.head.clone()
            })
        })
    }
    fn push<'a>(
        &'a self,
        _git: &'a Git,
        _folder: &'a Path,
        args: Vec<OsString>,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let mut state = self.0.lock().expect("fixture");
            assert!(state.head.is_none());
            assert!(
                args.iter()
                    .any(|arg| arg == "--force-with-lease=refs/heads/ghost/feature:")
            );
            state.head = Some(
                args.last()
                    .expect("refspec")
                    .to_string_lossy()
                    .split(':')
                    .next()
                    .expect("commit")
                    .into(),
            );
            state.pushes += 1;
            Ok(())
        })
    }
}

async fn fixture() -> (Rig, ManagedWorktree, FixturePublisher) {
    let rig = rig().await;
    let record = isolated(&rig).await;
    rig.git
        .run(
            &rig.work,
            args(&[
                "remote",
                "add",
                "origin",
                "https://github.com/fixture/project.git",
            ]),
            8192,
        )
        .await
        .expect("owned remote config only");
    commit_file(&rig, Path::new(&record.path), "reviewed committed change\n").await;
    let publisher = FixturePublisher(Mutex::new(State {
        base: record.base_commit.clone(),
        head: None,
        created: false,
        pushes: 0,
        bodies: vec![],
        creates: 0,
    }));
    (rig, record, publisher)
}

fn input() -> PullRequestInput {
    PullRequestInput {
        title: "Reviewed change".into(),
        body: "First paragraph.\n\nExact multiline body.\n".into(),
        base_branch: None,
    }
}

#[tokio::test]
async fn reviewed_pr_creation_is_one_use_and_reuses_the_existing_result() {
    let (rig, record, publisher) = fixture().await;
    let preview = rig
        .service
        .pull_request_preview_with("chat", &record.id, input(), &publisher)
        .await
        .expect("preview");
    assert!(preview.will_push && preview.existing_url.is_none());
    assert!(!preview.diff_stat.is_empty());
    assert_eq!(publisher.0.lock().expect("state").pushes, 0);
    let result = rig
        .service
        .pull_request_apply_with("chat", &preview.token, &publisher)
        .await
        .expect("apply");
    assert!(result.created && result.warning.is_none());
    assert_eq!(result.head_commit, preview.head_commit);
    assert_eq!(
        publisher.0.lock().expect("state").bodies,
        [input().body.into_bytes()]
    );
    assert!(
        rig.service
            .pull_request_apply_with("chat", &preview.token, &publisher)
            .await
            .is_err()
    );
    let again = rig
        .service
        .pull_request_preview_with("chat", &record.id, input(), &publisher)
        .await
        .expect("existing preview");
    assert!(!again.will_push);
    assert_eq!(again.existing_url.as_deref(), Some(result.url.as_str()));
    assert!(
        !rig.service
            .pull_request_apply_with("chat", &again.token, &publisher)
            .await
            .expect("reuse")
            .created
    );
    let state = publisher.0.lock().expect("state");
    assert_eq!((state.pushes, state.creates), (1, 1));
}

#[tokio::test]
async fn changed_local_commit_or_remote_base_prevents_publication() {
    let (rig, record, publisher) = fixture().await;
    let preview = rig
        .service
        .pull_request_preview_with("chat", &record.id, input(), &publisher)
        .await
        .expect("preview");
    commit_file(&rig, Path::new(&record.path), "newer local work\n").await;
    assert!(
        rig.service
            .pull_request_apply_with("chat", &preview.token, &publisher)
            .await
            .expect_err("stale local")
            .to_string()
            .contains("changed after")
    );
    let preview = rig
        .service
        .pull_request_preview_with("chat", &record.id, input(), &publisher)
        .await
        .expect("new preview");
    publisher.0.lock().expect("state").base = "f".repeat(40);
    assert!(
        rig.service
            .pull_request_apply_with("chat", &preview.token, &publisher)
            .await
            .expect_err("stale base")
            .to_string()
            .contains("base branch changed")
    );
    let state = publisher.0.lock().expect("state");
    assert_eq!((state.pushes, state.creates), (0, 0));
}

#[tokio::test]
async fn an_existing_different_remote_head_is_never_overwritten() {
    let (rig, record, publisher) = fixture().await;
    let preview = rig
        .service
        .pull_request_preview_with("chat", &record.id, input(), &publisher)
        .await
        .expect("preview");
    publisher.0.lock().expect("state").head = Some("e".repeat(40));
    assert!(
        rig.service
            .pull_request_apply_with("chat", &preview.token, &publisher)
            .await
            .expect_err("changed remote")
            .to_string()
            .contains("different commit")
    );
    assert_eq!(publisher.0.lock().expect("state").pushes, 0);
}

#[tokio::test]
async fn the_native_creation_lease_never_changes_an_existing_remote_ref() {
    let rig = rig().await;
    let remote = tempfile::tempdir().expect("owned bare remote");
    rig.git
        .run(remote.path(), args(&["init", "--bare"]), 8192)
        .await
        .expect("bare fixture");
    let destination = super::super::git::path_argument(remote.path())
        .to_string_lossy()
        .into_owned();
    let before = inspect(&rig.git, &rig.work)
        .await
        .expect("before")
        .view
        .head
        .expect("head");
    rig.git
        .run(
            &rig.work,
            push_arguments(&destination, "ghost/reviewed", &before),
            8192,
        )
        .await
        .expect("create only");
    commit_file(&rig, &rig.work, "later commit\n").await;
    let after = inspect(&rig.git, &rig.work)
        .await
        .expect("after")
        .view
        .head
        .expect("head");
    assert!(
        rig.git
            .run(
                &rig.work,
                push_arguments(&destination, "ghost/reviewed", &after),
                8192
            )
            .await
            .is_err()
    );
    assert_eq!(
        remote_head(&rig.git, &rig.work, &destination, "ghost/reviewed", &[])
            .await
            .expect("remote head"),
        Some(before)
    );
}

#[test]
fn remote_and_result_urls_are_bound_to_the_reviewed_github_repository() {
    for remote in [
        "https://github.com/fixture/project.git",
        "git@github.com:fixture/project.git",
        "ssh://git@github.com/fixture/project.git",
    ] {
        assert_eq!(
            repository_from_remote(remote).expect("repo"),
            "fixture/project"
        );
    }
    for remote in [
        "https://secret@github.com/fixture/project",
        "https://github.com.evil/fixture/project",
        "https://github.com/fixture/project\nhttps://github.com/other/repo",
        "-flag",
        "https://github.com/fixture/../project",
    ] {
        assert!(repository_from_remote(remote).is_err());
    }
    assert!(
        validate_pr_url(
            "fixture/project",
            "https://github.com/fixture/project/pull/17"
        )
        .is_ok()
    );
    for url in [
        "https://other.invalid/fixture/project/pull/17",
        "https://github.com/other/repo/pull/17",
        "https://github.com/fixture/project/pull/17?redirect=x",
    ] {
        assert!(validate_pr_url("fixture/project", url).is_err());
    }
    for name in [
        "-main",
        "main..other",
        "main@{0}",
        "main.lock",
        "heads//main",
    ] {
        assert!(validate_branch(name).is_err());
    }
}
