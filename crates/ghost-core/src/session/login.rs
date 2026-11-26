//! Explicit vendor terminals: sign-in, account tokens and updates. No vendor command runs while
//! inspecting this plan; only the terminal the person starts runs it.
use super::{SessionError, SessionManager};
use ghost_agents::maintenance::{
    MaintenanceCommand, MaintenanceError, check_unchanged, login_command, token_command,
    update_command,
};
use ghost_process::{LaunchSpec, ProcessCwd, SpawnSpec};
use ghost_protocol::agents::AgentInstallation;
use std::path::PathBuf;
use std::time::Duration;

impl SessionManager {
    /// Resolve only the already-detected native installation. The terminal owner must invoke this
    /// for an explicit user action, then launch the returned argv without shell interpolation.
    pub async fn prepare_login(&self, agent_id: &str) -> Result<SpawnSpec, SessionError> {
        self.prepare_vendor_terminal(agent_id, "sign-in", login_command)
            .await
    }

    /// `claude setup-token` for an account added in Ghost: the browser sign-in that prints the
    /// token the person pastes into Add account. Like the sign-in, only an explicit Start runs it.
    pub async fn prepare_account_token(&self, agent_id: &str) -> Result<SpawnSpec, SessionError> {
        self.prepare_vendor_terminal(agent_id, "account token", token_command)
            .await
    }

    /// The agent's own update command (the one Update runs in the background), in a terminal the
    /// person watches: when the background update fails, its output and any prompt it asks show
    /// here instead of being lost. The same channel rules apply; nothing runs that Update would not.
    pub async fn prepare_agent_update(&self, agent_id: &str) -> Result<SpawnSpec, SessionError> {
        // The same command replaces the same files: it waits for working chats as Update does.
        self.refuse_update_while_working(agent_id).await?;
        let env = self.deps().env.get().await;
        let mut spec = self
            .prepare_vendor_terminal(agent_id, "update", |installation| {
                update_command(installation, &env.vars)
            })
            .await?;
        spec.launch.startup_timeout = Duration::from_secs(30);
        Ok(spec)
    }

    async fn prepare_vendor_terminal(
        &self,
        agent_id: &str,
        purpose: &str,
        command: impl FnOnce(&AgentInstallation) -> Result<MaintenanceCommand, MaintenanceError>,
    ) -> Result<SpawnSpec, SessionError> {
        if self.shared().is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        let detector = &self.deps().detector;
        if detector.agent(agent_id).is_none() {
            return Err(SessionError::UnknownAgent(agent_id.into()));
        }
        // Detected again, not read from the cache: a failed update or a settings change drops the
        // cached detection, and the terminal then refused to start until someone refreshed.
        let (name, installation) = detector
            .fresh_installation(agent_id)
            .await
            .map_err(SessionError::AgentUnavailable)?;
        let command = command(&installation)
            .map_err(|error| SessionError::AgentUnavailable(error.to_string()))?;
        let env = self.deps().env.get().await;
        let folder = env
            .vars
            .get(&if cfg!(windows) { "USERPROFILE" } else { "HOME" }.into())
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or_else(|| {
                SessionError::AgentUnavailable("The home folder is unavailable.".into())
            })?;
        // This checks executable identity only, never credentials or vendor configuration.
        check_unchanged(&installation)
            .map_err(|error| SessionError::AgentUnavailable(error.to_string()))?;
        Ok(SpawnSpec {
            launch: LaunchSpec {
                label: format!("{name} {purpose}"),
                program: command.program,
                args: command.args,
                cwd: ProcessCwd::Fixed(folder.clone()),
                set_env: Vec::new(),
                daemons: Vec::new(),
                startup_timeout: Duration::from_secs(15),
            },
            folder,
            env,
            max_line_bytes: ghost_process::MAX_LINE_BYTES,
        })
    }
}
