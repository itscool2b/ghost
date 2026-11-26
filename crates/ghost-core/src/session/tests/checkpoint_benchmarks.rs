//! Opt-in measurement of the real actor send path through a scripted native driver.
use super::*;

#[tokio::test]
#[ignore = "manual native prompt dispatch performance fixture"]
async fn checkpoint_prompt_performance() {
    let mut steps = vec![ready_step("checkpoint-benchmark")];
    for _ in 0..3 {
        steps.extend([
            Step::Expect(CommandMatch::Prompt),
            Step::EndTurn {
                stop_reason: StopReason::EndTurn,
                usage: None,
                error: None,
            },
        ]);
    }
    steps.extend([
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ]);
    let (driver, handle) = ScriptedDriver::new(steps);
    let mut rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    enable(&mut rig, true);
    for index in 0..1_000 {
        std::fs::write(
            rig.folder.join(format!("file-{index:05}.txt")),
            format!(
                "fixture {index}\r\n{}",
                "line of synthetic source\n".repeat(10)
            ),
        )
        .unwrap();
    }
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    for attempt in 0..3 {
        if attempt == 2 {
            std::fs::write(rig.folder.join("file-00000.txt"), "changed\r\n").unwrap();
        }
        let started = std::time::Instant::now();
        rig.manager
            .send(&chat.id, "benchmark prompt".into())
            .await
            .unwrap();
        tokio::time::timeout(
            Duration::from_secs(120),
            handle.wait_for(CommandMatch::Prompt),
        )
        .await
        .expect("driver receives checkpointed prompt");
        println!(
            "CHECKPOINT_PROMPT files=1000 scenario={} send_to_driver_ms={}",
            match attempt {
                0 => "cold",
                1 => "unchanged",
                _ => "one-file",
            },
            started.elapsed().as_millis()
        );
        status(&rig, &chat.id, SessionStatus::Ready).await;
    }
    rig.manager.stop(&chat.id).await.unwrap();
}
