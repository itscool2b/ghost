//! Personal vendor operations prepared without starting an agent. Start consumes the review and
//! rechecks both executable bytes and selected public declarations under desktop mutation admission.
use super::{SessionError, SessionManager};
use ghost_agents::config_vendor::{VendorPlan, prepare};
use ghost_config::{
    Kind, Library, Provider, Scope, Source,
    declarations::{self, Declaration, DocumentFormat},
};
use ghost_process::{LaunchSpec, ProcessCwd, SpawnSpec};
use ghost_protocol::{
    config_vendor::{VendorKind, VendorProvider, VendorRequest, VendorReview},
    session::Availability,
};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    time::{Duration, Instant},
};

pub struct PreparedVendor {
    review: VendorReview,
    created: Instant,
}
impl PreparedVendor {
    pub fn review(&self) -> &VendorReview {
        &self.review
    }
    pub fn expired(&self) -> bool {
        self.created.elapsed() >= Duration::from_secs(600)
    }
}
pub struct VendorLaunch {
    pub spec: SpawnSpec,
    /// Transfer this into TerminalTicket::start_guarded. Never drop it while the child is alive.
    pub guard: Box<dyn Send + Sync>,
}
struct Owner(SessionManager);
impl Drop for Owner {
    fn drop(&mut self) {
        self.0.shared().live.end_maintenance();
    }
}
fn unavailable() -> SessionError {
    SessionError::Organization("The selected integration or configuration changed or cannot be safely inspected. Refresh the Library and review again.".into())
}

impl SessionManager {
    pub async fn review_config_vendor(
        &self,
        request: VendorRequest,
    ) -> Result<PreparedVendor, SessionError> {
        let (plan, _) = self.vendor_plan(request).await?;
        Ok(PreparedVendor {
            review: plan.review,
            created: Instant::now(),
        })
    }
    pub async fn start_config_vendor(
        &self,
        pending: PreparedVendor,
    ) -> Result<VendorLaunch, SessionError> {
        if pending.expired() || self.shared().is_shutting_down() {
            return Err(unavailable());
        }
        if !self.shared().live.begin_maintenance() {
            return Err(SessionError::ShuttingDown);
        }
        let owner = Owner(self.clone());
        if self.shared().is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        let (plan, env) = self.vendor_plan(pending.review.request.clone()).await?;
        if pending.expired() || plan.review.revision != pending.review.revision {
            return Err(unavailable());
        }
        Ok(VendorLaunch {
            spec: SpawnSpec {
                launch: LaunchSpec {
                    label: format!("{} integration", plan.review.request.provider.agent_id()),
                    program: plan.program,
                    args: plan.args,
                    cwd: ProcessCwd::Fixed(plan.folder.clone()),
                    set_env: plan.set_env,
                    daemons: Vec::new(),
                    startup_timeout: Duration::from_secs(15),
                },
                folder: plan.folder,
                env,
                max_line_bytes: ghost_process::MAX_LINE_BYTES,
            },
            guard: Box::new(owner),
        })
    }
    async fn vendor_plan(
        &self,
        request: VendorRequest,
    ) -> Result<(VendorPlan, std::sync::Arc<ghost_process::ResolvedEnv>), SessionError> {
        if self.shared().is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        let agent = self
            .deps()
            .detector
            .cached()
            .into_iter()
            .find(|agent| agent.id == request.provider.agent_id())
            .ok_or_else(unavailable)?;
        if !matches!(agent.availability, Availability::Ready { .. }) {
            return Err(unavailable());
        }
        let installation = agent.installation.ok_or_else(unavailable)?;
        let env = self.deps().env.get().await;
        let inspected_env = env.clone();
        let plan = self
            .deps()
            .rt
            .spawn_blocking(move || {
                let mut plan = prepare(&installation, &inspected_env.vars, &request)
                    .map_err(|error| SessionError::Organization(error.to_string()))?;
                let state = declaration_revision(&plan)?;
                let mut hash = Sha256::new();
                hash.update(plan.review.revision.as_bytes());
                hash.update(state.as_bytes());
                plan.review.revision = format!("{:x}", hash.finalize());
                Ok::<_, SessionError>(plan)
            })
            .await
            .map_err(|_| unavailable())??;
        Ok((plan, env))
    }
}

fn declaration_revision(plan: &VendorPlan) -> Result<String, SessionError> {
    if plan.review.request.kind == VendorKind::Native {
        return Ok(format!(
            "native:{}:{}",
            plan.review.request.provider.agent_id(),
            plan.config.display()
        ));
    }
    let (provider, path, format, fields) = match plan.review.request.provider {
        VendorProvider::Claude => (
            Provider::Claude,
            "settings.json",
            DocumentFormat::Json,
            vec!["enabledPlugins", "extraKnownMarketplaces"],
        ),
        VendorProvider::Codex => (
            Provider::Codex,
            "config.toml",
            DocumentFormat::Toml,
            vec!["plugins", "marketplaces"],
        ),
        VendorProvider::Pi => (
            Provider::Pi,
            "settings.json",
            DocumentFormat::Json,
            vec!["packages"],
        ),
        _ => return Err(unavailable()),
    };
    let source = Source {
        id: format!("user:{}", plan.review.request.provider.agent_id()),
        provider,
        scope: Scope::User,
        root: plan.config.clone(),
        claude_state_only: false,
    };
    let mut hash = Sha256::new();
    if missing(&plan.config)? {
        if plan.review.request.item_revision.is_some() {
            return Err(unavailable());
        }
        hash.update(b"missing-root");
    } else {
        for field in fields {
            // A document that cannot be read is never taken for one without the declaration.
            match declarations::read(&source, path, format, field).map_err(|_| unavailable())? {
                Declaration::Absent => hash.update(b"absent"),
                Declaration::Present(bytes) => {
                    // Pi's exact source may be an object with resource filters. Removing or
                    // updating is still delegated to Pi; no filtered declaration is rewritten here.
                    hash.update((bytes.len() as u64).to_le_bytes());
                    hash.update(bytes.as_slice());
                }
            }
        }
        if let Some(expected) = &plan.review.request.item_revision {
            let kind = match plan.review.request.kind {
                VendorKind::Plugin => Kind::Plugin,
                VendorKind::Package => Kind::Package,
                _ => return Err(unavailable()),
            };
            let inventory = Library::new(vec![source])
                .map_err(|_| unavailable())?
                .inspect();
            if !inventory.items.iter().any(|item| {
                item.item.kind == kind
                    && item.item.key == plan.review.request.target
                    && &item.revision == expected
            }) {
                return Err(unavailable());
            }
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn missing(path: &Path) -> Result<bool, SessionError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.is_symlink() => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        _ => Err(unavailable()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghost_protocol::config_vendor::{VendorAction, VendorKind};
    fn plan(root: &Path) -> VendorPlan {
        VendorPlan {
            review: VendorReview {
                token: String::new(),
                request: VendorRequest {
                    provider: VendorProvider::Claude,
                    kind: VendorKind::Plugin,
                    action: VendorAction::Install,
                    target: "tools@team".into(),
                    item_revision: None,
                },
                program: "fixture".into(),
                arguments: vec![],
                working_directory: root.display().to_string(),
                config_directory: root.display().to_string(),
                revision: "fixture".into(),
                timeout_seconds: 600,
                notices: vec![],
            },
            program: root.join("fixture"),
            args: vec![],
            folder: root.into(),
            config: root.join(".claude"),
            set_env: vec![],
        }
    }
    #[test]
    fn public_declaration_changes_invalidate_review_but_private_settings_are_not_bound() {
        let temp = tempfile::tempdir().unwrap();
        let plan = plan(temp.path());
        let absent = declaration_revision(&plan).unwrap();
        std::fs::create_dir(&plan.config).unwrap();
        std::fs::write(
            plan.config.join("settings.json"),
            r#"{"enabledPlugins":{"tools@team":true},"env":{"TOKEN":"private-one"}}"#,
        )
        .unwrap();
        let first = declaration_revision(&plan).unwrap();
        assert_ne!(absent, first);
        std::fs::write(
            plan.config.join("settings.json"),
            r#"{"enabledPlugins":{"tools@team":true},"env":{"TOKEN":"private-two"}}"#,
        )
        .unwrap();
        assert_eq!(first, declaration_revision(&plan).unwrap());
        std::fs::write(
            plan.config.join("settings.json"),
            r#"{"enabledPlugins":{"tools@team":false}}"#,
        )
        .unwrap();
        assert_ne!(first, declaration_revision(&plan).unwrap());
        std::fs::write(plan.config.join("settings.json"), "broken").unwrap();
        assert!(declaration_revision(&plan).is_err());
    }

    #[test]
    fn selected_item_revision_and_codex_marketplace_changes_are_bound() {
        let temp = tempfile::tempdir().unwrap();
        let mut plan = plan(temp.path());
        std::fs::create_dir(&plan.config).unwrap();
        std::fs::write(
            plan.config.join("settings.json"),
            r#"{"enabledPlugins":{"tools@team":true}}"#,
        )
        .unwrap();
        plan.review.request.item_revision = Some("f".repeat(64));
        assert!(declaration_revision(&plan).is_err());
        plan.review.request.item_revision = None;
        plan.review.request.provider = VendorProvider::Codex;
        std::fs::write(
            plan.config.join("config.toml"),
            "[marketplaces.team]\nsource = 'owner/repository'\n",
        )
        .unwrap();
        let first = declaration_revision(&plan).unwrap();
        std::fs::write(
            plan.config.join("config.toml"),
            "[marketplaces.team]\nsource = 'owner/replacement'\n",
        )
        .unwrap();
        assert_ne!(first, declaration_revision(&plan).unwrap());
        let expired = PreparedVendor {
            review: plan.review,
            created: Instant::now() - Duration::from_secs(600),
        };
        assert!(expired.expired());
    }
}
