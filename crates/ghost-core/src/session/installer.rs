//! Native install review and revalidation. Starting a terminal remains a separate user action.
use super::{SessionError, SessionManager};
use ghost_agents::installer::{InstallPlan, prepare};
use ghost_process::{LaunchSpec, ProcessCwd, SpawnSpec};
use ghost_protocol::{session::Availability, setup_actions::AgentInstallReview};
use std::time::Duration;

impl SessionManager {
    async fn install_plan(
        &self,
        agent_id: &str,
        refresh: bool,
    ) -> Result<(InstallPlan, std::sync::Arc<ghost_process::ResolvedEnv>), SessionError> {
        if self.shared().is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        let agents = self.agents(refresh).await;
        let agent = agents
            .into_iter()
            .find(|agent| agent.id == agent_id)
            .ok_or_else(|| SessionError::UnknownAgent(agent_id.into()))?;
        if !matches!(agent.availability, Availability::Missing { .. })
            || agent.installation.is_some()
        {
            return Err(SessionError::AgentUnavailable("Installation is only offered for a missing native agent. Refresh detection before trying again.".into()));
        }
        let env = self.deps().env.get().await;
        let inspected = std::sync::Arc::clone(&env);
        let agent_id = agent_id.to_owned();
        let plan = tokio::task::spawn_blocking(move || prepare(&agent_id, &inspected.vars))
            .await
            .map_err(|_| SessionError::Internal("The installer inspection failed.".into()))?
            .map_err(|error| SessionError::AgentUnavailable(error.to_string()))?;
        Ok((plan, env))
    }

    /// No installation, login or model command runs during review.
    pub async fn review_agent_install(
        &self,
        agent_id: &str,
    ) -> Result<AgentInstallReview, SessionError> {
        Ok(self.install_plan(agent_id, false).await?.0.review)
    }

    /// The actual executable, argv and folder must still match what the user reviewed. Refreshing
    /// availability prevents an old missing-agent screen from becoming an unintended update.
    pub async fn prepare_agent_install(
        &self,
        agent_id: &str,
        revision: &str,
    ) -> Result<SpawnSpec, SessionError> {
        let (plan, env) = self.install_plan(agent_id, true).await?;
        if plan.review.revision != revision {
            return Err(SessionError::AgentUnavailable(
                "The installer changed. Review the installation again.".into(),
            ));
        }
        Ok(SpawnSpec {
            launch: LaunchSpec {
                label: format!("{agent_id} installation"),
                program: plan.program,
                args: plan.arguments,
                cwd: ProcessCwd::Fixed(plan.folder.clone()),
                set_env: Vec::new(),
                daemons: Vec::new(),
                startup_timeout: Duration::from_secs(15),
            },
            folder: plan.folder,
            env,
            max_line_bytes: ghost_process::MAX_LINE_BYTES,
        })
    }
}
