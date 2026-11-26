use super::*;
use ghost_protocol::agents::AgentInstallations;

impl SessionManager {
    pub async fn agent_installations(
        &self,
        agent_id: &str,
    ) -> Result<AgentInstallations, SessionError> {
        self.deps()
            .detector
            .installations(agent_id)
            .await
            .map_err(SessionError::Organization)
    }
    pub async fn select_agent_installation(
        &self,
        agent_id: &str,
        path: Option<String>,
    ) -> Result<AgentInstallations, SessionError> {
        if self.shared().is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        self.deps()
            .detector
            .select_installation(agent_id, path)
            .await
            .map_err(SessionError::Organization)
    }
}
