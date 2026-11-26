//! `StandardDriverFactory`: the ACP driver for `DriverKind::Acp` and the demo agent's driver for
//! `DriverKind::Mock`.

use std::sync::Arc;

use ghost_protocol::DriverKind;

use crate::acp::AcpDriver;
use crate::driver::{AgentDriver, DriverFactory};
use crate::mock_driver::{MockBehavior, MockDriver};

/// The drivers Ghost ships.
#[derive(Debug, Clone, Copy, Default)]
pub struct StandardDriverFactory;

impl StandardDriverFactory {
    pub fn new() -> Arc<StandardDriverFactory> {
        Arc::new(StandardDriverFactory)
    }
}

impl DriverFactory for StandardDriverFactory {
    fn create(&self, kind: DriverKind) -> Box<dyn AgentDriver> {
        match kind {
            DriverKind::Acp => Box::new(AcpDriver::new()),
            DriverKind::Claude => Box::new(crate::claude::ClaudeDriver::new()),
            DriverKind::Codex => Box::new(crate::codex::CodexDriver::new()),
            DriverKind::Headless => Box::new(crate::headless::HeadlessDriver),
            DriverKind::Pi => Box::new(crate::pi::PiDriver::new()),
            DriverKind::Mock => Box::new(MockDriver::new(MockBehavior::default())),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::time::Duration;

    use futures::future::BoxFuture;
    use ghost_process::testing::FakeSpawner;
    use ghost_process::{
        EnvMap, EnvSource, LaunchSpec, ProcessCwd, ProcessSpawner, ProtocolLog, ResolvedEnv,
        SpawnError, SpawnSpec, SpawnedProcess,
    };
    use ghost_protocol::Autonomy;
    use tokio::sync::mpsc;

    use super::*;
    use crate::driver::{
        AgentSpawner, DriverContext, DriverEvent, DriverTimings, SessionSetup, event_channel,
    };

    /// Spawns through `FakeSpawner` with a log that writes nothing, so the test leaves no file.
    struct QuietSpawner(Arc<FakeSpawner>);

    impl AgentSpawner for QuietSpawner {
        fn spawn(&self, spec: SpawnSpec) -> BoxFuture<'static, Result<SpawnedProcess, SpawnError>> {
            self.0.spawn(spec, ProtocolLog::disabled())
        }
    }

    fn setup(dialect: &str) -> SessionSetup {
        SessionSetup {
            mcp_servers: Vec::new(),
            attachment_dir: None,
            attachment_writer: None,
            chat_id: "chat".into(),
            agent_id: dialect.into(),
            dialect: dialect.into(),
            folder: PathBuf::from("/work"),
            agent_folder: "/work".into(),
            add_dirs: Vec::new(),
            launch: LaunchSpec {
                label: dialect.into(),
                program: PathBuf::from("/opt/bin/agent"),
                args: Vec::new(),
                cwd: ProcessCwd::ChatFolder,
                set_env: Vec::new(),
                daemons: Vec::new(),
                startup_timeout: Duration::from_secs(60),
            },
            env: Arc::new(ResolvedEnv {
                vars: EnvMap::new(),
                source: EnvSource::Fallback {
                    reason: "test".into(),
                },
                warnings: Vec::new(),
                resolved_at_ms: 0,
            }),
            resume: None,
            native_start: Default::default(),
            autonomy: Autonomy::FullAuto,
            desired: BTreeMap::new(),
            timings: DriverTimings::STANDARD,
            client_version: "0.1.0".into(),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn each_kind_gets_its_driver() {
        let factory = StandardDriverFactory::new();

        // The demo agent reaches Ready without a process.
        let (spawner, mut ends) = FakeSpawner::new();
        let (_commands, receiver) = mpsc::unbounded_channel();
        let (sender, mut events) = event_channel();
        tokio::spawn(factory.create(DriverKind::Mock).run(DriverContext {
            commands: receiver,
            events: sender,
            spawner: Arc::new(QuietSpawner(Arc::clone(&spawner))),
            setup: setup("mock"),
        }));
        loop {
            match events.recv().await.expect("an event") {
                DriverEvent::Ready(info) => {
                    assert_eq!(info.agent_name.as_deref(), Some("Demo agent"));
                    break;
                }
                _ => continue,
            }
        }
        assert!(ends.try_recv().is_err(), "the demo agent spawns nothing");

        // The ACP driver spawns its agent and starts with `initialize`.
        let (_commands, receiver) = mpsc::unbounded_channel();
        let (sender, _events) = event_channel();
        tokio::spawn(factory.create(DriverKind::Acp).run(DriverContext {
            commands: receiver,
            events: sender,
            spawner: Arc::new(QuietSpawner(spawner)),
            setup: setup("hermes"),
        }));
        let mut end = ends.recv().await.expect("spawned");
        let line = end.from_client.recv().await.expect("a line");
        let message: serde_json::Value = serde_json::from_str(&line).expect("json");
        assert_eq!(message["method"], "initialize");
    }
}
