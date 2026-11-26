use super::*;
use futures::future::BoxFuture;
use std::sync::Mutex;

struct Fixture(Mutex<FixtureState>);
#[derive(Default)]
struct FixtureState {
    exists: bool,
    repo_id: u64,
    private: bool,
    branch: Option<String>,
    creates: usize,
    pushes: usize,
    lose_create: bool,
    lose_push: bool,
    deny_reads: bool,
}
impl Fixture {
    fn new() -> Self {
        Self(Mutex::new(FixtureState {
            repo_id: 17,
            private: true,
            ..Default::default()
        }))
    }
}
fn metadata(state: &FixtureState) -> serde_json::Value {
    serde_json::json!({"id":state.repo_id,"full_name":"fixture/project","html_url":"https://github.com/fixture/project","private":state.private,"archived":false,"fork":false,"permissions":{"push":true}})
}
impl Remote for Fixture {
    fn request<'a>(
        &'a self,
        _folder: &'a Path,
        method: &'a str,
        endpoint: &'a str,
        fields: Vec<(String, String)>,
    ) -> BoxFuture<'a, Result<Response>> {
        Box::pin(async move {
            let mut state = self.0.lock().unwrap();
            if method == "GET" && state.deny_reads {
                return Ok(Response {
                    status: 403,
                    body: serde_json::json!({"message":"denied"}),
                });
            }
            match (method, endpoint) {
                ("GET", "user") => Ok(Response {
                    status: 200,
                    body: serde_json::json!({"login":"fixture","type":"User"}),
                }),
                ("GET", "repos/fixture/project") => Ok(Response {
                    status: if state.exists { 200 } else { 404 },
                    body: metadata(&state),
                }),
                ("GET", "repos/fixture/project/git/ref/heads/main") => Ok(Response {
                    status: if state.branch.is_some() { 200 } else { 404 },
                    body: serde_json::json!({"ref":"refs/heads/main","object":{"type":"commit","sha":state.branch}}),
                }),
                ("POST", "user/repos") => {
                    assert!(
                        !state.exists,
                        "an existing repository must not be recreated"
                    );
                    assert_eq!(fields, vec![("name".into(), "project".into())]);
                    state.creates += 1;
                    state.exists = true;
                    if state.lose_create {
                        state.lose_create = false;
                        return Err(invalid("lost create response"));
                    }
                    Ok(Response {
                        status: 201,
                        body: metadata(&state),
                    })
                }
                _ => panic!("unexpected request {method} {endpoint}"),
            }
        })
    }
    fn push<'a>(
        &'a self,
        _git: &'a Git,
        _folder: &'a Path,
        url: &'a str,
        branch: &'a str,
        head: &'a str,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            assert_eq!(url, "https://github.com/fixture/project.git");
            assert_eq!(branch, "main");
            let mut state = self.0.lock().unwrap();
            assert!(state.exists && state.private);
            assert!(state.branch.is_none());
            state.pushes += 1;
            state.branch = Some(head.into());
            if state.lose_push {
                state.lose_push = false;
                return Err(invalid("lost push response"));
            }
            Ok(())
        })
    }
}
fn input() -> Input {
    Input {
        owner: String::new(),
        name: "project".into(),
    }
}

#[tokio::test]
async fn no_commit_guides_explicit_local_setup_before_any_remote_creation() {
    let rig = super::super::tests::rig().await;
    let remote = Fixture::new();
    let empty = rig.work.parent().unwrap().join("empty");
    std::fs::create_dir(&empty).unwrap();
    let error = rig
        .service
        .publication_review_with("empty", &empty, input(), &remote)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Initialize"));
    rig.git
        .run(&empty, args(&["init", "-b", "main"]), 8192)
        .await
        .unwrap();
    std::fs::write(empty.join("untracked"), "keep me").unwrap();
    let error = rig
        .service
        .publication_review_with("empty", &empty, input(), &remote)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("first commit"));
    assert!(!empty.join(".git/index").exists());
    assert_eq!(
        std::fs::read_to_string(empty.join("untracked")).unwrap(),
        "keep me"
    );
    let state = remote.0.lock().unwrap();
    assert_eq!((state.creates, state.pushes), (0, 0));
}

#[tokio::test]
async fn private_publication_preserves_mixed_index_worktree_and_reuses_confirmed_result() {
    let rig = super::super::tests::rig().await;
    let remote = Fixture::new();
    std::fs::write(rig.work.join("file.txt"), "staged\n").unwrap();
    rig.git
        .run(&rig.work, args(&["add", "--", "file.txt"]), 8192)
        .await
        .unwrap();
    std::fs::write(rig.work.join("file.txt"), "unstaged\n").unwrap();
    std::fs::write(rig.work.join("untracked"), "keep\n").unwrap();
    let index = std::fs::read(rig.work.join(".git/index")).unwrap();
    let review = rig
        .service
        .publication_review_with("folder", &rig.work, input(), &remote)
        .await
        .unwrap();
    assert!(review.private && review.will_create && review.will_push && !review.recovering);
    assert_eq!(remote.0.lock().unwrap().creates, 0);
    let result = rig
        .service
        .publication_apply_with("folder", &review.token, &remote)
        .await
        .unwrap();
    assert_eq!(result.state, State::Complete);
    assert_eq!(std::fs::read(rig.work.join(".git/index")).unwrap(), index);
    assert_eq!(
        std::fs::read_to_string(rig.work.join("file.txt")).unwrap(),
        "unstaged\n"
    );
    assert_eq!(
        std::fs::read_to_string(rig.work.join("untracked")).unwrap(),
        "keep\n"
    );
    assert_eq!(
        origin(&rig.git, &rig.work).await.unwrap().as_deref(),
        Some("https://github.com/fixture/project.git")
    );
    assert!(
        rig.service
            .publication_apply_with("folder", &review.token, &remote)
            .await
            .is_err()
    );
    let again = rig
        .service
        .publication_review_with("folder", &rig.work, input(), &remote)
        .await
        .unwrap();
    assert!(!again.will_create && !again.will_push && again.recovering);
    assert_eq!(
        rig.service
            .publication_apply_with("folder", &again.token, &remote)
            .await
            .unwrap()
            .state,
        State::Complete
    );
    let state = remote.0.lock().unwrap();
    assert_eq!((state.creates, state.pushes), (1, 1));
}

#[tokio::test]
async fn lost_create_and_push_responses_require_new_review_without_duplicate_remote_mutations() {
    let rig = super::super::tests::rig().await;
    let remote = Fixture::new();
    remote.0.lock().unwrap().lose_create = true;
    let review = rig
        .service
        .publication_review_with("folder", &rig.work, input(), &remote)
        .await
        .unwrap();
    assert_eq!(
        rig.service
            .publication_apply_with("folder", &review.token, &remote)
            .await
            .unwrap()
            .state,
        State::NeedsReview
    );
    assert!(origin(&rig.git, &rig.work).await.unwrap().is_none());
    assert_eq!(
        rig.service.read_publications().unwrap()[0].phase,
        Phase::Creating
    );
    let recovery = rig
        .service
        .publication_review_with("folder", &rig.work, input(), &remote)
        .await
        .unwrap();
    assert!(recovery.recovering && !recovery.will_create);
    remote.0.lock().unwrap().lose_push = true;
    assert_eq!(
        rig.service
            .publication_apply_with("folder", &recovery.token, &remote)
            .await
            .unwrap()
            .state,
        State::NeedsReview
    );
    assert!(origin(&rig.git, &rig.work).await.unwrap().is_none());
    let recovery = rig
        .service
        .publication_review_with("folder", &rig.work, input(), &remote)
        .await
        .unwrap();
    assert!(!recovery.will_push);
    assert_eq!(
        rig.service
            .publication_apply_with("folder", &recovery.token, &remote)
            .await
            .unwrap()
            .state,
        State::Complete
    );
    let state = remote.0.lock().unwrap();
    assert_eq!((state.creates, state.pushes), (1, 1));
}

#[tokio::test]
async fn changed_index_private_visibility_and_repository_identity_are_rejected() {
    let rig = super::super::tests::rig().await;
    let remote = Fixture::new();
    let review = rig
        .service
        .publication_review_with("folder", &rig.work, input(), &remote)
        .await
        .unwrap();
    std::fs::write(rig.work.join("file.txt"), "new staged\n").unwrap();
    rig.git
        .run(&rig.work, args(&["add", "--", "file.txt"]), 8192)
        .await
        .unwrap();
    assert!(
        rig.service
            .publication_apply_with("folder", &review.token, &remote)
            .await
            .is_err()
    );
    assert_eq!(remote.0.lock().unwrap().creates, 0);
    let review = rig
        .service
        .publication_review_with("folder", &rig.work, input(), &remote)
        .await
        .unwrap();
    remote.0.lock().unwrap().lose_push = true;
    assert_eq!(
        rig.service
            .publication_apply_with("folder", &review.token, &remote)
            .await
            .unwrap()
            .state,
        State::NeedsReview
    );
    remote.0.lock().unwrap().private = false;
    assert!(
        rig.service
            .publication_review_with("folder", &rig.work, input(), &remote)
            .await
            .is_err()
    );
    {
        let mut state = remote.0.lock().unwrap();
        state.private = true;
        state.repo_id = 99;
    }
    assert!(
        rig.service
            .publication_review_with("folder", &rig.work, input(), &remote)
            .await
            .is_err()
    );
    assert!(origin(&rig.git, &rig.work).await.unwrap().is_none());
}

#[tokio::test]
async fn existing_remote_or_auth_failure_never_creates_or_pushes() {
    let rig = super::super::tests::rig().await;
    let remote = Fixture::new();
    remote.0.lock().unwrap().exists = true;
    assert!(
        rig.service
            .publication_review_with("folder", &rig.work, input(), &remote)
            .await
            .is_err()
    );
    {
        let mut state = remote.0.lock().unwrap();
        state.exists = false;
        state.deny_reads = true;
    }
    assert!(
        rig.service
            .publication_review_with("folder", &rig.work, input(), &remote)
            .await
            .is_err()
    );
    {
        let state = remote.0.lock().unwrap();
        assert_eq!((state.creates, state.pushes), (0, 0));
    }
    remote.0.lock().unwrap().deny_reads = false;
    rig.git
        .run(
            &rig.work,
            args(&[
                "remote",
                "add",
                "origin",
                "https://github.com/fixture/existing.git",
            ]),
            8192,
        )
        .await
        .unwrap();
    assert!(
        rig.service
            .publication_review_with("folder", &rig.work, input(), &remote)
            .await
            .is_err()
    );
    assert_eq!(
        origin(&rig.git, &rig.work).await.unwrap().as_deref(),
        Some("https://github.com/fixture/existing.git")
    );
}
