//! The profile's added agent accounts ([`crate::agent_accounts`]): the list the Agents page shows,
//! and moving a chat to another account where its limit stopped it.
use super::{SessionError, SessionManager, actor::ChatMsg, lock};
use crate::agent_accounts::{AccountError, AgentAccountsStore};
use ghost_protocol::agents::{AgentAccounts, AgentUsage};
use std::sync::Arc;

const UNAVAILABLE: &str = "Added accounts are not available in this build.";

impl SessionManager {
    /// Gives agents the profile's added accounts. Without them (tests, the CLI) every agent signs
    /// in on its own.
    pub fn set_agent_accounts(&self, store: Option<Arc<AgentAccountsStore>>) {
        *lock(&self.shared().agent_accounts) = store;
    }

    /// Keeps the usage book in `path` (a local profile's `agent-usage.json`) across restarts.
    pub fn persist_agent_usage(&self, path: &std::path::Path) {
        self.shared().agent_usage.persist_at(path);
    }

    /// What each of the agent's accounts last reported about its usage limits.
    pub fn agent_usage(&self, agent_id: &str) -> AgentUsage {
        self.shared()
            .agent_usage
            .usage(ghost_agents::agent_of(agent_id))
    }

    pub async fn agent_accounts(&self, agent_id: &str) -> Result<AgentAccounts, SessionError> {
        let agent = ghost_agents::agent_of(agent_id).to_owned();
        if self.shared().agent_accounts().is_none() {
            return Ok(AgentAccounts {
                agent_id: agent,
                accounts: Vec::new(),
                active: None,
                unavailable: Some(UNAVAILABLE.into()),
            });
        }
        self.with_accounts(move |store| store.list(&agent)).await
    }

    /// Adds an account from the token `claude setup-token` printed. It is not used until chosen.
    pub async fn add_agent_account(
        &self,
        agent_id: &str,
        name: String,
        token: String,
    ) -> Result<AgentAccounts, SessionError> {
        let agent = ghost_agents::agent_of(agent_id).to_owned();
        let now = crate::time::now_millis();
        self.with_accounts(move |store| store.add(&agent, &name, &token, now))
            .await
    }

    /// Removes an account and its token. Agents running on it keep their process until it stops.
    pub async fn remove_agent_account(
        &self,
        agent_id: &str,
        account_id: String,
    ) -> Result<AgentAccounts, SessionError> {
        let agent = ghost_agents::agent_of(agent_id).to_owned();
        let removed = account_id.clone();
        let accounts = self
            .with_accounts({
                let agent = agent.clone();
                move |store| store.remove(&agent, &account_id)
            })
            .await?;
        if !accounts
            .accounts
            .iter()
            .any(|account| account.id == removed)
        {
            self.shared().agent_usage.forget(&agent, &removed);
        }
        Ok(accounts)
    }

    /// Chooses the account the agent's next starts sign in with (`None`: its own sign-in).
    /// Running agents keep theirs until they start again.
    pub async fn use_agent_account(
        &self,
        agent_id: &str,
        account_id: Option<String>,
    ) -> Result<AgentAccounts, SessionError> {
        let agent = ghost_agents::agent_of(agent_id).to_owned();
        self.with_accounts(move |store| store.activate(&agent, account_id.as_deref()))
            .await
    }

    /// "Continue with ...": chooses `account_id` for the agent, then starts `chat_id`'s agent again
    /// on it, resuming the same session, and sends the prompts held there. Other chats keep the
    /// account they run on until they start again.
    pub async fn switch_chat_account(
        &self,
        chat_id: &str,
        agent_id: &str,
        account_id: Option<String>,
    ) -> Result<AgentAccounts, SessionError> {
        let accounts = self.use_agent_account(agent_id, account_id).await?;
        let agent = accounts.agent_id.clone();
        self.ask(chat_id, |reply| ChatMsg::RestartOnAccount {
            agent: agent.clone(),
            reply,
        })
        .await?;
        Ok(accounts)
    }

    async fn with_accounts<T: Send + 'static>(
        &self,
        work: impl FnOnce(&AgentAccountsStore) -> Result<T, AccountError> + Send + 'static,
    ) -> Result<T, SessionError> {
        if self.shared().is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        let store = self
            .shared()
            .agent_accounts()
            .ok_or_else(|| SessionError::AgentUnavailable(UNAVAILABLE.into()))?;
        tokio::task::spawn_blocking(move || work(&store))
            .await
            .map_err(|_| SessionError::Internal("the account change stopped".into()))?
            .map_err(account_error)
    }
}

fn account_error(error: AccountError) -> SessionError {
    match error {
        AccountError::Invalid(_) | AccountError::Unknown => {
            SessionError::OptionInvalid(error.to_string())
        }
        AccountError::Unavailable(message) | AccountError::Vault(message) => {
            SessionError::AgentUnavailable(message)
        }
        AccountError::Storage(_) => SessionError::Organization(error.to_string()),
    }
}
