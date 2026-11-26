use super::*;
use crate::driver::{DriverError, FileRewindResult, LoggedCommand};
use ghost_protocol::ErrorKind;
use tokio::sync::oneshot;

const CHECKPOINT: &str = "019a0000-0000-7000-8000-000000000009";

async fn enabled() -> Harness {
    let mut settings = setup();
    settings.native_start.file_checkpointing = true;
    let mut h = Harness::start_with(settings).await;
    h.ready().await;
    h
}

fn rewind(
    h: &Harness,
    id: &str,
    dry_run: bool,
) -> oneshot::Receiver<Result<FileRewindResult, DriverError>> {
    let (reply, receive) = oneshot::channel();
    let command = DriverCommand::RewindFiles {
        user_message_id: id.into(),
        dry_run,
        reply,
    };
    assert_eq!(
        serde_json::to_value(LoggedCommand::from(&command)).unwrap(),
        json!({"type":"rewind_files","user_message_id":id,"dry_run":dry_run}),
    );
    h.tx.send(command).unwrap();
    receive
}

#[tokio::test(start_paused = true)]
async fn rewind_requires_opt_in_valid_checkpoint_and_idle_without_requests() {
    let mut h = Harness::start().await;
    h.ready().await;
    assert!(rewind(&h, CHECKPOINT, false).await.unwrap().is_err());
    assert!(
        timeout(Duration::from_millis(5), h.end.from_client.recv())
            .await
            .is_err()
    );
    h.stop().await;

    let mut h = enabled().await;
    assert!(rewind(&h, "invalid", true).await.unwrap().is_err());
    h.prompt();
    h.wire().await;
    assert!(rewind(&h, CHECKPOINT, false).await.unwrap().is_err());
    h.send(json!({"type":"result","subtype":"success","user_message_uuid":"t1"}))
        .await;
    h.send(json!({"type":"system","subtype":"session_state_changed","state":"idle"}))
        .await;
    h.send(json!({"type":"control_request","request_id":"pending","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"echo hello"}}})).await;
    loop {
        if matches!(h.event().await, DriverEvent::Permission { .. }) {
            break;
        }
    }
    assert!(rewind(&h, CHECKPOINT, false).await.unwrap().is_err());
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn previews_and_real_rewinds_preserve_result_statistics_and_link_skips() {
    let mut h = enabled().await;
    for dry_run in [true, false] {
        let receive = rewind(&h, CHECKPOINT, dry_run);
        let frame = h.wire().await;
        assert_eq!(
            frame["request"],
            json!({"subtype":"rewind_files","user_message_id":CHECKPOINT,"dry_run":dry_run})
        );
        h.send(json!({"type":"control_response","response":{"subtype":"success","request_id":frame["request_id"],"response":{"canRewind":true,"filesChanged":["src/main.rs"],"insertions":2,"deletions":3,"skippedLinks":1,"future":"ignored"}}})).await;
        let result = receive.await.unwrap().unwrap();
        assert_eq!(result.files_changed, ["src/main.rs"]);
        assert_eq!(
            (result.insertions, result.deletions, result.skipped_links),
            (Some(2), Some(3), Some(1))
        );
    }
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn inner_failure_and_malformed_preview_are_errors_without_stopping_the_session() {
    let mut h = enabled().await;
    for response in [
        json!({"canRewind":false,"error":"No file checkpoint found for this message."}),
        json!({}),
        json!({"canRewind":true,"filesChanged":12}),
    ] {
        let receive = rewind(&h, CHECKPOINT, true);
        let frame = h.wire().await;
        h.send(json!({"type":"control_response","response":{"subtype":"success","request_id":frame["request_id"],"response":response}})).await;
        assert!(receive.await.unwrap().is_err());
    }
    h.prompt();
    assert_eq!(h.wire().await["type"], "user");
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_prompt_waits_for_file_rewind_confirmation() {
    let mut h = enabled().await;
    let receive = rewind(&h, CHECKPOINT, false);
    let frame = h.wire().await;
    h.prompt();
    assert!(
        timeout(Duration::from_millis(5), h.end.from_client.recv())
            .await
            .is_err()
    );
    h.send(json!({"type":"control_response","response":{"subtype":"success","request_id":frame["request_id"],"response":{"canRewind":true}}})).await;
    receive.await.unwrap().unwrap();
    assert_eq!(h.wire().await["type"], "user");
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn failed_real_rewind_holds_prompts_because_files_may_have_changed() {
    for inner_failure in [false, true] {
        let mut h = enabled().await;
        let receive = rewind(&h, CHECKPOINT, false);
        let frame = h.wire().await;
        h.prompt();
        let response = if inner_failure {
            json!({"subtype":"success","request_id":frame["request_id"],"response":{"canRewind":false,"error":"Restore failed"}})
        } else {
            json!({"subtype":"error","request_id":frame["request_id"],"error":"Restore failed"})
        };
        h.send(json!({"type":"control_response","response":response}))
            .await;
        assert!(receive.await.unwrap().is_err());
        assert_eq!(
            h.end.from_client.recv().await,
            None,
            "held prompt never reaches Claude"
        );
        while h.events.recv().await.is_some() {}
        assert!(matches!(h.run.await.unwrap(), DriverExit::Crashed { .. }));
    }
}

#[tokio::test(start_paused = true)]
async fn timed_out_preview_can_retry_but_unknown_real_rewind_closes_without_sending_held_prompt() {
    let mut h = enabled().await;
    let receive = rewind(&h, CHECKPOINT, true);
    h.wire().await;
    tokio::time::advance(Duration::from_secs(31)).await;
    assert!(receive.await.unwrap().is_err());
    h.prompt();
    assert_eq!(h.wire().await["type"], "user");
    h.stop().await;

    for malformed_reply in [false, true] {
        let mut h = enabled().await;
        let receive = rewind(&h, CHECKPOINT, false);
        let frame = h.wire().await;
        h.prompt();
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        if malformed_reply {
            h.send(json!({"type":"control_response","response":{"subtype":"success","request_id":frame["request_id"],"response":{}}})).await;
        } else {
            tokio::time::advance(Duration::from_secs(31)).await;
        }
        assert_eq!(
            receive.await.unwrap().unwrap_err().kind,
            ErrorKind::ProtocolError
        );
        assert_eq!(
            h.end.from_client.recv().await,
            None,
            "held prompt never reaches Claude"
        );
        while h.events.recv().await.is_some() {}
        assert!(matches!(h.run.await.unwrap(), DriverExit::Crashed { .. }));
    }
}
