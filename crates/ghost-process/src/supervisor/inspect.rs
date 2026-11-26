//! Read-only discovery facts resolved on Linux. Exports paths, never an environment map or the
//! contents of provider settings, credentials or transcripts.

use std::collections::BTreeMap;
use std::io;

use serde::{Deserialize, Serialize};

use super::environment::LaunchEnvironment;
use super::wire::{PROTOCOL_VERSION, invalid};
use crate::ResolvedEnv;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectRequest {
    pub protocol: u32,
    pub cwd: String,
    #[serde(default)]
    pub set_env: LaunchEnvironment,
    /// Bare executable names selected by the catalog. Discovery never executes them.
    pub executables: Vec<String>,
}

impl InspectRequest {
    pub fn validate(&self) -> io::Result<()> {
        self.set_env.validate()?;
        if self.protocol != PROTOCOL_VERSION
            || !self.cwd.starts_with('/')
            || self.cwd.contains('\0')
            || self.executables.len() > 64
            || self.executables.iter().any(|name| {
                name.is_empty()
                    || name.len() > 256
                    || name.starts_with('-')
                    || name
                        .chars()
                        .any(|c| c.is_control() || matches!(c, '/' | '\\'))
                    || matches!(name.as_str(), "." | "..")
            })
        {
            return Err(invalid("invalid supervisor inspection request"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutablePath {
    pub name: String,
    /// Keep the user's stable symlink as the launch path.
    pub launch_path: String,
    pub resolved_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostInspection {
    pub protocol: u32,
    pub canonical_cwd: String,
    /// Known provider configuration roots only. Their spelling remains Linux case sensitive.
    pub config_roots: BTreeMap<String, String>,
    pub executables: Vec<ExecutablePath>,
}

/// Proof scoped to the current supervisor connection, not a reusable deployment capability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prepared {
    pub preparation: String,
    pub inspection: HostInspection,
}

pub(crate) struct Preparation {
    request: InspectRequest,
    prepared: Prepared,
}

impl Preparation {
    pub(crate) fn new(request: InspectRequest, env: &ResolvedEnv) -> io::Result<Self> {
        let inspection = inspect(&request, env)?;
        Ok(Self {
            request,
            prepared: Prepared {
                preparation: uuid::Uuid::now_v7().to_string(),
                inspection,
            },
        })
    }

    pub(crate) fn response(&self) -> &Prepared {
        &self.prepared
    }

    pub(crate) fn verify(
        &self,
        launch: &super::wire::LaunchRequest,
        env: &ResolvedEnv,
    ) -> io::Result<()> {
        if launch.preparation.as_deref() != Some(self.prepared.preparation.as_str()) {
            return Err(invalid(
                "supervisor preparation does not match this connection",
            ));
        }
        self.verify_program(&launch.program)?;
        // Final driver flags may be added after discovery. Recompute the actual realm with all
        // final overrides instead of assuming those flags cannot change HOME, PATH or a root.
        let request = InspectRequest {
            cwd: launch.cwd.clone(),
            set_env: launch.set_env.clone(),
            ..self.request.clone()
        };
        self.verify_snapshot(&inspect(&request, env)?)
    }

    fn verify_program(&self, program: &str) -> io::Result<()> {
        if !self
            .prepared
            .inspection
            .executables
            .iter()
            .any(|executable| {
                program == executable.launch_path
                    || Some(program) == executable.resolved_path.as_deref()
            })
        {
            return Err(invalid("supervisor launch program was not inspected"));
        }
        Ok(())
    }

    fn verify_snapshot(&self, current: &HostInspection) -> io::Result<()> {
        if current != &self.prepared.inspection {
            return Err(invalid(
                "supervisor host paths or configuration realm changed after inspection",
            ));
        }
        Ok(())
    }
}

/// Blocking metadata work, called on the supervisor's blocking pool. No CLI or shell is started.
pub fn inspect(request: &InspectRequest, env: &ResolvedEnv) -> io::Result<HostInspection> {
    request.validate()?;
    #[cfg(target_os = "linux")]
    {
        linux(request, &request.set_env.effective(env))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = env;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "host inspection requires Linux",
        ))
    }
}

#[cfg(target_os = "linux")]
fn linux(request: &InspectRequest, env: &ResolvedEnv) -> io::Result<HostInspection> {
    use crate::{EnvKey, find_program};
    use std::path::{Component, Path, PathBuf};

    fn text(path: &Path) -> io::Result<String> {
        path.to_str()
            .map(ToOwned::to_owned)
            .ok_or_else(|| invalid("host path is not UTF-8"))
    }
    fn identity(path: PathBuf) -> io::Result<String> {
        let mut canonical = None;
        for ancestor in path.ancestors() {
            match std::fs::canonicalize(ancestor) {
                Ok(root) => {
                    let tail = path.strip_prefix(ancestor).map_err(io::Error::other)?;
                    canonical = Some(root.join(tail));
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    // A dangling symlink is not a missing lexical suffix. Its target realm is
                    // unknown, so skipping it would falsely identify the alias as the root.
                    match std::fs::symlink_metadata(ancestor) {
                        Ok(_) => return Err(invalid("host configuration root cannot be resolved")),
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                        Err(_) => {
                            return Err(invalid("host configuration root cannot be inspected"));
                        }
                    }
                }
                Err(_) => return Err(invalid("host configuration root cannot be inspected")),
            }
        }
        let path =
            canonical.ok_or_else(|| invalid("host configuration root cannot be resolved"))?;
        let mut normalized = PathBuf::new();
        for component in path.components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    normalized.pop();
                }
                other => normalized.push(other.as_os_str()),
            }
        }
        text(&normalized)
    }
    let cwd = std::fs::canonicalize(&request.cwd)?;
    if !cwd.is_dir() {
        return Err(invalid("host working directory is not a directory"));
    }
    let variable = |name: &str| {
        env.vars
            .get(&EnvKey::from(name))
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let mut config_roots = BTreeMap::new();
    for (provider, variable_name, default) in [
        ("claude", "CLAUDE_CONFIG_DIR", ".claude"),
        ("codex", "CODEX_HOME", ".codex"),
    ] {
        if let Some(path) =
            variable(variable_name).or_else(|| variable("HOME").map(|home| home.join(default)))
        {
            let absolute = if path.is_absolute() {
                path
            } else {
                cwd.join(path)
            };
            config_roots.insert(provider.into(), identity(absolute)?);
        }
    }
    let mut executables = Vec::new();
    for name in &request.executables {
        if let Some(path) = find_program(name, &env.vars) {
            executables.push(ExecutablePath {
                name: name.clone(),
                launch_path: text(&path)?,
                resolved_path: std::fs::canonicalize(&path)
                    .ok()
                    .and_then(|path| text(&path).ok()),
            });
        }
    }
    Ok(HostInspection {
        protocol: PROTOCOL_VERSION,
        canonical_cwd: text(&cwd)?,
        config_roots,
        executables,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preparation_snapshot_rejects_changed_cwd_roots_and_executable_identity() {
        let snapshot = HostInspection {
            protocol: PROTOCOL_VERSION,
            canonical_cwd: "/work".into(),
            config_roots: [("claude".into(), "/home/user/.claude".into())].into(),
            executables: vec![ExecutablePath {
                name: "claude".into(),
                launch_path: "/bin/claude".into(),
                resolved_path: Some("/opt/claude-1".into()),
            }],
        };
        let preparation = Preparation {
            request: InspectRequest {
                protocol: PROTOCOL_VERSION,
                cwd: "/work".into(),
                set_env: Default::default(),
                executables: vec!["claude".into()],
            },
            prepared: Prepared {
                preparation: uuid::Uuid::now_v7().to_string(),
                inspection: snapshot.clone(),
            },
        };
        preparation.verify_snapshot(&snapshot).expect("unchanged");
        preparation
            .verify_program("/bin/claude")
            .expect("stable path");
        preparation
            .verify_program("/opt/claude-1")
            .expect("resolved path");
        assert!(preparation.verify_program("/bin/uninspected").is_err());
        for field in ["cwd", "root", "executable"] {
            let mut changed = snapshot.clone();
            match field {
                "cwd" => changed.canonical_cwd = "/other".into(),
                "root" => {
                    changed
                        .config_roots
                        .insert("claude".into(), "/other-config".into());
                }
                _ => changed.executables[0].resolved_path = Some("/opt/claude-2".into()),
            }
            assert!(preparation.verify_snapshot(&changed).is_err(), "{field}");
        }
    }

    #[test]
    fn inspection_accepts_only_bounded_catalog_names_and_linux_folders() {
        let mut request = InspectRequest {
            protocol: PROTOCOL_VERSION,
            cwd: "/workspace".into(),
            set_env: LaunchEnvironment::default(),
            executables: vec!["claude".into()],
        };
        request.validate().expect("valid");
        for name in ["../agent", "/usr/bin/agent", "a\nb", "-x", ".."] {
            request.executables = vec![name.into()];
            assert!(request.validate().is_err());
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn inspection_preserves_linux_case_and_exports_no_credential_environment() {
        use crate::{EnvKey, EnvSource};
        let dir = tempfile::tempdir().expect("folder");
        let alias = dir.path().join("alias");
        std::os::unix::fs::symlink(dir.path(), &alias).expect("alias");
        let env = ResolvedEnv {
            vars: [
                (EnvKey::from("HOME"), dir.path().as_os_str().to_owned()),
                (
                    EnvKey::from("CLAUDE_CONFIG_DIR"),
                    alias.join("NewConfig").into_os_string(),
                ),
                (
                    EnvKey::from("ANTHROPIC_API_KEY"),
                    "private-value-never-exported".into(),
                ),
            ]
            .into(),
            source: EnvSource::Fallback {
                reason: "test".into(),
            },
            warnings: Vec::new(),
            resolved_at_ms: 0,
        };
        let request = InspectRequest {
            protocol: PROTOCOL_VERSION,
            cwd: alias.to_string_lossy().into_owned(),
            set_env: LaunchEnvironment(vec![("CODEX_HOME".into(), "./PreparedConfig".into())]),
            executables: Vec::new(),
        };
        let result = inspect(&request, &env).expect("inspection");
        assert_eq!(result.canonical_cwd, dir.path().to_string_lossy());
        assert!(result.config_roots["claude"].ends_with("/NewConfig"));
        assert_eq!(
            result.config_roots["codex"],
            dir.path().join("PreparedConfig").to_string_lossy()
        );
        let body = serde_json::to_string(&result).expect("json");
        assert!(!body.contains("ANTHROPIC_API_KEY"));
        assert!(!body.contains("private-value"));

        let dangling = dir.path().join("dangling");
        std::os::unix::fs::symlink(dir.path().join("missing-target"), &dangling)
            .expect("dangling alias");
        let request = InspectRequest {
            set_env: LaunchEnvironment(vec![(
                "CODEX_HOME".into(),
                dangling.to_string_lossy().into_owned(),
            )]),
            ..request
        };
        assert!(
            inspect(&request, &env).is_err(),
            "unknown target must not become a distinct realm"
        );
    }
}
