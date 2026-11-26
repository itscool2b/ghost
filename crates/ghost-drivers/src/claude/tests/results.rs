use super::*;

#[tokio::test(start_paused = true)]
async fn streamed_stop_reasons_survive_results_without_overriding_errors_or_interrupts() {
    for (reason, result, expected) in [
        (
            "max_tokens",
            json!({"subtype":"success"}),
            StopReason::MaxTokens,
        ),
        ("refusal", json!({"subtype":"success"}), StopReason::Refusal),
        (
            "max_tokens",
            json!({"subtype":"success","stop_reason":"end_turn"}),
            StopReason::EndTurn,
        ),
        (
            "max_tokens",
            json!({"is_error":true,"terminal_reason":"api_error"}),
            StopReason::Error,
        ),
        (
            "refusal",
            json!({"terminal_reason":"aborted_streaming"}),
            StopReason::Cancelled,
        ),
    ] {
        let mut h = Harness::start().await;
        h.ready().await;
        h.prompt();
        h.wire().await;
        h.send(
            json!({"type":"stream_event","event":{"type":"message_start","message":{"id":"main"}}}),
        )
        .await;
        h.send(json!({"type":"stream_event","event":{"type":"message_delta","delta":{"stop_reason":reason}}})).await;
        let mut result = result;
        result["type"] = json!("result");
        result["user_message_uuid"] = json!("t1");
        h.send(result).await;
        loop {
            if let DriverEvent::TurnEnded { stop_reason, .. } = h.event().await {
                assert_eq!(stop_reason, expected);
                break;
            }
        }
        h.stop().await;
    }
}
