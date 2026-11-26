//! The Agents page's added accounts and the chat's account switch, over ghost-core's accounts
//! (docs/agent-accounts.md). A token goes from the page straight into secure storage: no command
//! returns one, and errors never carry one.
use crate::agent_tokens::{NativeTokens, TokenEntry, TokenError};
use crate::{
    error::{AppError, AppErrorCode},
    state::AppState,
};
use ghost_core::agent_accounts::TokenVault;
use ghost_protocol::agents::{AgentAccounts, AgentUsage};
use tauri::{State, Webview};

/// Credential Manager or the keychain: one entry per added account.
pub struct ProfileTokens {
    store: NativeTokens,
}

impl ProfileTokens {
    /// The reason instead, when this computer has no secure storage Ghost can use.
    pub fn open() -> Result<Self, String> {
        let store = NativeTokens::new().map_err(|_| {
            "This computer has no secure storage Ghost can keep account tokens in.".to_owned()
        })?;
        Ok(Self { store })
    }

    fn entry(&self, account: &str) -> Result<TokenEntry, String> {
        TokenEntry::new(account).map_err(|_| "The account id is invalid.".to_owned())
    }
}

impl TokenVault for ProfileTokens {
    fn load(&self, account: &str) -> Result<Option<String>, String> {
        let entry = self.entry(account)?;
        let Some(secret) = self.store.load(&entry).map_err(vault_message)? else {
            return Ok(None);
        };
        String::from_utf8(secret.to_vec())
            .map(Some)
            .map_err(|_| TokenError::Corrupt.to_string())
    }

    fn save(&self, account: &str, token: &str) -> Result<(), String> {
        let entry = self.entry(account)?;
        self.store
            .save(&entry, token.as_bytes())
            .map_err(vault_message)
    }

    fn remove(&self, account: &str) -> Result<(), String> {
        let entry = self.entry(account)?;
        self.store.remove(&entry).map_err(vault_message)
    }
}

fn vault_message(error: TokenError) -> String {
    error.to_string()
}

fn current(view: &Webview, state: &AppState) -> Result<(), AppError> {
    if state.quit.is_quitting() || !state.pages.for_view(view.label()).is_current() {
        return Err(AppError::new(
            AppErrorCode::SessionNotReady,
            "This page is no longer active.",
        ));
    }
    Ok(())
}

fn stopped() -> AppError {
    AppError::new(
        AppErrorCode::Internal,
        "The account change could not be finished.",
    )
}

#[tauri::command]
#[specta::specta]
pub async fn agent_accounts(
    webview: Webview,
    state: State<'_, AppState>,
    agent_id: String,
) -> Result<AgentAccounts, AppError> {
    current(&webview, &state)?;
    Ok(state.sessions()?.agent_accounts(&agent_id).await?)
}

/// What each of the agent's accounts last reported about its usage limits.
#[tauri::command]
#[specta::specta]
pub fn agent_usage(
    webview: Webview,
    state: State<'_, AppState>,
    agent_id: String,
) -> Result<AgentUsage, AppError> {
    current(&webview, &state)?;
    Ok(state.sessions()?.agent_usage(&agent_id))
}

/// Adds an account from the token `claude setup-token` printed. The account is not used yet.
#[tauri::command]
#[specta::specta]
pub async fn agent_account_add(
    webview: Webview,
    state: State<'_, AppState>,
    agent_id: String,
    name: String,
    token: String,
) -> Result<AgentAccounts, AppError> {
    current(&webview, &state)?;
    let sessions = state.sessions()?;
    // Saving continues if the page closes, so a stored token is always listed.
    tokio::spawn(async move { Ok(sessions.add_agent_account(&agent_id, name, token).await?) })
        .await
        .map_err(|_| stopped())?
}

#[tauri::command]
#[specta::specta]
pub async fn agent_account_remove(
    webview: Webview,
    state: State<'_, AppState>,
    agent_id: String,
    account_id: String,
) -> Result<AgentAccounts, AppError> {
    current(&webview, &state)?;
    let sessions = state.sessions()?;
    tokio::spawn(async move { Ok(sessions.remove_agent_account(&agent_id, account_id).await?) })
        .await
        .map_err(|_| stopped())?
}

/// The account the agent's next starts sign in with; `None`: its own sign-in.
#[tauri::command]
#[specta::specta]
pub async fn agent_account_use(
    webview: Webview,
    state: State<'_, AppState>,
    agent_id: String,
    account_id: Option<String>,
) -> Result<AgentAccounts, AppError> {
    current(&webview, &state)?;
    let sessions = state.sessions()?;
    tokio::spawn(async move { Ok(sessions.use_agent_account(&agent_id, account_id).await?) })
        .await
        .map_err(|_| stopped())?
}

/// "Continue with ...": the account for the agent, then this chat starts again on it, resuming
/// its session, and sends what was held.
#[tauri::command]
#[specta::specta]
pub async fn chat_switch_account(
    webview: Webview,
    state: State<'_, AppState>,
    chat_id: String,
    agent_id: String,
    account_id: Option<String>,
) -> Result<AgentAccounts, AppError> {
    current(&webview, &state)?;
    let sessions = state.sessions()?;
    tokio::spawn(async move {
        Ok(sessions
            .switch_chat_account(&chat_id, &agent_id, account_id)
            .await?)
    })
    .await
    .map_err(|_| stopped())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_errors_read_as_sentences_and_never_name_a_token() {
        for error in [
            TokenError::Unavailable,
            TokenError::Invalid,
            TokenError::Corrupt,
        ] {
            let message = vault_message(error);
            assert!(message.ends_with('.'), "{message}");
            assert!(!message.contains("sk-ant"), "{message}");
        }
        assert_eq!(
            vault_message(TokenError::Corrupt),
            "The stored token is damaged. Remove the account and add it again."
        );
        assert_eq!(
            vault_message(TokenError::Unavailable),
            "Secure storage is unavailable or locked. Unlock it and try again."
        );
    }
}
