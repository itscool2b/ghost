//! Library roots come from the same resolved environment as agents and persisted project folders.
//! Configuration mutation stays owned through its journal even after the webview caller leaves.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use ghost_config::{CopyResult, Library, PreparedCopy, Provider, Scope, Source};
use ghost_process::{EnvKey, EnvMap};

use super::{SessionError, SessionManager};

pub struct LibraryRoots {
    pub sources: Vec<Source>,
    pub notices: Vec<String>,
}

impl SessionManager {
    pub async fn config_library_roots(
        &self,
        folder_id: Option<&str>,
    ) -> Result<LibraryRoots, SessionError> {
        let folder = match folder_id {
            Some(id) => {
                let preview = self.chat_create_preview(id).await?;
                Some((
                    id.to_owned(),
                    PathBuf::from(preview.folder.path.ok_or_else(|| {
                        SessionError::Organization(
                            "This project's local folder is unavailable.".into(),
                        )
                    })?),
                ))
            }
            None => None,
        };
        let env = self.deps().env.get().await;
        Ok(roots(
            &env.vars,
            folder
                .as_ref()
                .map(|(id, path)| (id.as_str(), path.as_path())),
        ))
    }

    pub async fn config_library_apply(
        &self,
        library: Arc<Library>,
        prepared: PreparedCopy,
    ) -> Result<CopyResult, SessionError> {
        let manager = self.clone();
        self.deps().rt.spawn_blocking(move || {
            let _owner = manager.shared().live.try_maintenance().ok_or(SessionError::ShuttingDown)?;
            if manager.shared().is_shutting_down() { return Err(SessionError::ShuttingDown); }
            library.apply_copy(prepared).map_err(config_error)
        }).await.map_err(|_| SessionError::Internal("The configuration copy stopped unexpectedly. Its recovery journal is retained.".into()))?
    }

    pub async fn config_library_rollback(
        &self,
        library: Arc<Library>,
        source_id: String,
        id: String,
    ) -> Result<CopyResult, SessionError> {
        let manager = self.clone();
        self.deps().rt.spawn_blocking(move || {
            let _owner = manager.shared().live.try_maintenance().ok_or(SessionError::ShuttingDown)?;
            if manager.shared().is_shutting_down() { return Err(SessionError::ShuttingDown); }
            library.rollback_copy(&source_id, &id).map_err(config_error)
        }).await.map_err(|_| SessionError::Internal("Configuration recovery stopped unexpectedly. All retained files remain available.".into()))?
    }
}

fn config_error(error: ghost_config::Error) -> SessionError {
    SessionError::Organization(error.to_string())
}

pub(super) fn roots(env: &EnvMap, folder: Option<(&str, &Path)>) -> LibraryRoots {
    // Consult only these named configuration paths. Do not enumerate/export the login environment.
    let variable = |key: &str| {
        env.get(&EnvKey::from(key))
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let home = if cfg!(windows) {
        variable("USERPROFILE").or_else(|| variable("HOME"))
    } else {
        variable("HOME")
    }
    .filter(|path| path.is_absolute());
    let mut result = LibraryRoots {
        sources: Vec::new(),
        notices: Vec::new(),
    };
    for (name, provider, key, default) in [
        ("claude", Provider::Claude, "CLAUDE_CONFIG_DIR", ".claude"),
        ("codex", Provider::Codex, "CODEX_HOME", ".codex"),
        ("pi", Provider::Pi, "PI_CODING_AGENT_DIR", ".pi/agent"),
    ] {
        let candidate = variable(key).or_else(|| home.as_ref().map(|home| home.join(default)));
        let root = candidate.and_then(|path| {
            if path.is_absolute() {
                Some(path)
            } else {
                folder.map(|(_, folder)| folder.join(path))
            }
        });
        if let Some(root) = root {
            result.sources.push(Source {
                id: format!("user:{name}"),
                provider,
                scope: Scope::User,
                root,
                claude_state_only: false,
            });
        } else {
            result.notices.push(format!("{name}: the configured user folder is unavailable or relative. Select a project folder to resolve relative paths."));
        }
        if let Some((id, root)) = folder {
            result.sources.push(Source {
                id: format!("project:{id}:{name}"),
                provider,
                scope: Scope::Project,
                root: root.into(),
                claude_state_only: false,
            });
        }
    }
    if let Some(home) = home {
        result.sources.push(Source {
            id: "user:shared".into(),
            provider: Provider::Shared,
            scope: Scope::User,
            root: home.join(".agents/skills"),
            claude_state_only: false,
        });
        if variable("CLAUDE_CONFIG_DIR").is_none() {
            result.sources.push(Source {
                id: "user:claude-mcp".into(),
                provider: Provider::Claude,
                scope: Scope::User,
                root: home,
                claude_state_only: true,
            });
        } else {
            result.notices.push("User MCP inventory for a custom Claude home needs a verified state-file mapping. Project MCP inventory is available.".into());
        }
    }
    if let Some((id, root)) = folder {
        let shared = root.join(".agents/skills");
        if !result
            .sources
            .iter()
            .any(|source| source.provider == Provider::Shared && source.root == shared)
        {
            result.sources.push(Source {
                id: format!("project:{id}:shared"),
                provider: Provider::Shared,
                scope: Scope::Project,
                root: shared,
                claude_state_only: false,
            });
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_explicit_path_variables_become_roots_and_relative_overrides_need_a_folder() {
        let temp = tempfile::tempdir().unwrap();
        let env: EnvMap = [
            (EnvKey::from("HOME"), temp.path().into()),
            (EnvKey::from("USERPROFILE"), temp.path().into()),
            (EnvKey::from("CLAUDE_CONFIG_DIR"), "relative-claude".into()),
            (EnvKey::from("ANTHROPIC_API_KEY"), "SYNTHETIC_SECRET".into()),
        ]
        .into();
        let without = roots(&env, None);
        assert!(
            !without
                .sources
                .iter()
                .any(|source| source.id == "user:claude")
        );
        assert!(
            !without
                .sources
                .iter()
                .any(|source| source.claude_state_only)
        );
        let with = roots(&env, Some(("folder", temp.path())));
        assert_eq!(
            with.sources
                .iter()
                .find(|source| source.id == "user:claude")
                .unwrap()
                .root,
            temp.path().join("relative-claude")
        );
        assert!(
            with.sources
                .iter()
                .all(|source| !source.root.to_string_lossy().contains("SECRET"))
        );
        assert_eq!(
            with.sources
                .iter()
                .filter(|source| source.provider == Provider::Shared)
                .count(),
            1
        );
    }
}
