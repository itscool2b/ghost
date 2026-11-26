//! Execution-host boundary. A discovered WSL binary is not a safely supervised native process.
//!
//! `wsl.exe` in a Windows job does not put the Linux descendants in that job. Until a qualified
//! Linux supervisor is deployed, WSL remains explicitly unavailable instead of silently falling
//! back to a native launch or claiming that terminating the Windows relay stopped the agent.

use std::sync::Arc;

use futures::FutureExt;
use futures::future::BoxFuture;

use crate::{ProcessSpawner, ProtocolLog, SpawnError, SpawnSpec, SpawnedProcess};

/// Process-layer counterpart of the application's host identity, without a protocol dependency.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum HostExecution {
    #[default]
    Native,
    Wsl {
        distro: String,
    },
    Unsupported,
}

/// Whether this process layer has a qualified execution path for a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostAvailability {
    Available,
    Unavailable { reason: String },
}

impl HostExecution {
    pub fn availability(&self) -> HostAvailability {
        match self {
            Self::Native => HostAvailability::Available,
            Self::Wsl { distro } if !valid_distro(distro) => HostAvailability::Unavailable {
                reason: "the WSL distribution name is invalid".into(),
            },
            Self::Wsl { .. } => HostAvailability::Unavailable {
                reason: "a qualified Ghost Linux process supervisor is not deployed".into(),
            },
            Self::Unsupported => HostAvailability::Unavailable {
                reason: "this execution host is not supported by this version of Ghost".into(),
            },
        }
    }

    fn label(&self) -> String {
        match self {
            Self::Native => "native".into(),
            Self::Wsl { distro } if valid_distro(distro) => format!("WSL {distro}"),
            Self::Wsl { .. } => "WSL".into(),
            Self::Unsupported => "unsupported host".into(),
        }
    }
}

fn valid_distro(distro: &str) -> bool {
    !distro.trim().is_empty()
        && distro.len() <= 256
        && !distro.chars().any(char::is_control)
        && !distro.starts_with('-')
}

/// Selects the host before any subprocess is created. Native specs pass through unchanged.
pub struct HostSpawner {
    native: Arc<dyn ProcessSpawner>,
    host: HostExecution,
}

impl HostSpawner {
    pub fn new(native: Arc<dyn ProcessSpawner>, host: HostExecution) -> Arc<Self> {
        Arc::new(Self { native, host })
    }
}

impl ProcessSpawner for HostSpawner {
    fn spawn(
        &self,
        spec: SpawnSpec,
        log: ProtocolLog,
    ) -> BoxFuture<'static, Result<SpawnedProcess, SpawnError>> {
        match self.host.availability() {
            HostAvailability::Available => self.native.spawn(spec, log),
            HostAvailability::Unavailable { reason } => {
                let host = self.host.label();
                async move { Err(SpawnError::HostUnavailable { host, reason }) }.boxed()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use super::*;
    use crate::{EnvSource, LaunchSpec, ProcessCwd, ResolvedEnv, testing::FakeSpawner};

    fn spec() -> SpawnSpec {
        SpawnSpec {
            launch: LaunchSpec {
                label: "host test".into(),
                program: PathBuf::from("/linux/agent"),
                args: vec!["stdio".into()],
                cwd: ProcessCwd::ChatFolder,
                set_env: Vec::new(),
                daemons: Vec::new(),
                startup_timeout: Duration::from_secs(1),
            },
            folder: PathBuf::from("/linux/project"),
            env: Arc::new(ResolvedEnv {
                vars: Default::default(),
                source: EnvSource::Fallback {
                    reason: "test".into(),
                },
                warnings: Vec::new(),
                resolved_at_ms: 0,
            }),
            max_line_bytes: 1024,
        }
    }

    #[tokio::test]
    async fn native_delegates_the_original_launch_without_rewriting_paths() {
        let (native, mut processes) = FakeSpawner::new();
        let expected = spec();
        let process = HostSpawner::new(native, HostExecution::Native)
            .spawn(expected.clone(), ProtocolLog::disabled())
            .await
            .expect("native spawn");
        let child = processes.recv().await.expect("one spawn");
        assert_eq!(child.spec.launch, expected.launch);
        assert_eq!(child.spec.folder, expected.folder);
        drop(process);
    }

    #[tokio::test]
    async fn unavailable_hosts_never_start_a_native_process() {
        for host in [
            HostExecution::Unsupported,
            HostExecution::Wsl {
                distro: "Ubuntu".into(),
            },
            HostExecution::Wsl {
                distro: "-d other".into(),
            },
            HostExecution::Wsl {
                distro: "bad\nname".into(),
            },
        ] {
            let (native, mut processes) = FakeSpawner::new();
            let spawner = HostSpawner::new(native, host);
            let error = spawner
                .spawn(spec(), ProtocolLog::disabled())
                .await
                .expect_err("unavailable");
            assert!(matches!(error, SpawnError::HostUnavailable { .. }));
            assert!(processes.try_recv().is_err(), "no fallback or helper spawn");
        }
    }
}
