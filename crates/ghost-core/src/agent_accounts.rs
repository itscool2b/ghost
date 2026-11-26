//! Accounts an agent can run on besides its own sign-in (docs/agent-accounts.md).
//!
//! `agent-accounts.json` in a local profile's folder names the added accounts and says which one
//! agents start on. Each account's token lives in the system's secure storage ([`TokenVault`]),
//! never in a file. Only Claude Code takes added accounts: it reads a `claude setup-token` token
//! from `CLAUDE_CODE_OAUTH_TOKEN` and keeps every session in the same configuration folder,
//! whichever account runs it, so a chat continues on another account where it stopped. Codex keeps
//! each sign-in's threads in that sign-in's own home, so a second Codex account could not continue
//! a chat. Every call here does blocking I/O: run it on a blocking worker.

use crate::fsutil::{JsonFile, quarantine_file, read_json, write_json_atomic};
use ghost_protocol::Millis;
use ghost_protocol::agents::{AgentAccount, AgentAccounts};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

/// The agents that take added accounts.
pub const SUPPORTED: &[&str] = &["claude"];

/// Where Claude Code reads a `claude setup-token` token from.
pub const CLAUDE_TOKEN_VARIABLE: &str = "CLAUDE_CODE_OAUTH_TOKEN";

/// Credentials Claude Code prefers over [`CLAUDE_TOKEN_VARIABLE`], in its authentication
/// precedence. An added account's start leaves them out, so the account chosen is the one used.
/// `apiKeyHelper` and a settings file's own `env` still win; the docs say so.
pub const CLAUDE_OUTRANKING: &[&str] = &[
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_API_KEY",
];

const FILE: &str = "agent-accounts.json";
const VERSION: u32 = 1;
const MAX_NAME_CHARS: usize = 40;
const MAX_ACCOUNTS: usize = 16;
/// `claude setup-token` prints `sk-ant-oat01-` and about a hundred URL-safe characters.
const TOKEN_PREFIX: &str = "sk-ant-oat";
const TOKEN_CHARS: std::ops::RangeInclusive<usize> = 40..=512;

/// Why an account call failed. Messages are written to be shown as they are.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AccountError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Unavailable(String),
    #[error("That account is not in this profile anymore. Refresh and try again.")]
    Unknown,
    #[error("{0}")]
    Vault(String),
    #[error("The account list could not be saved. {0}")]
    Storage(String),
}

/// Secure storage for account tokens, keyed by account id. The desktop backs it with Credential
/// Manager or the keychain; errors are plain sentences without secret text.
pub trait TokenVault: Send + Sync {
    fn load(&self, account: &str) -> Result<Option<String>, String>;
    fn save(&self, account: &str, token: &str) -> Result<(), String>;
    fn remove(&self, account: &str) -> Result<(), String>;
}

/// What a start of an agent runs on: an added account and its token.
#[derive(Clone, PartialEq, Eq)]
pub struct AccountLaunch {
    pub account_id: String,
    pub name: String,
    pub token: String,
}

impl std::fmt::Debug for AccountLaunch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountLaunch")
            .field("account_id", &self.account_id)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Record {
    version: u32,
    #[serde(default)]
    agents: BTreeMap<String, AgentRecord>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct AgentRecord {
    #[serde(default)]
    active: Option<String>,
    #[serde(default)]
    accounts: Vec<AgentAccount>,
}

/// One profile's added accounts.
pub struct AgentAccountsStore {
    path: PathBuf,
    /// `Err`: why tokens cannot be stored here, so no account can be added or used.
    vault: Result<Arc<dyn TokenVault>, String>,
    lock: Mutex<()>,
}

impl std::fmt::Debug for AgentAccountsStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentAccountsStore")
            .field("path", &self.path)
            .field("vault", &self.vault.as_ref().map(|_| "available"))
            .finish()
    }
}

impl AgentAccountsStore {
    /// The accounts of the profile in `profile_dir`. `vault` is `Err` with the reason when this
    /// device or profile cannot keep tokens (no secure storage, an encrypted account profile).
    pub fn open(profile_dir: &Path, vault: Result<Arc<dyn TokenVault>, String>) -> Self {
        Self {
            path: profile_dir.join(FILE),
            vault,
            lock: Mutex::new(()),
        }
    }

    /// The account ids `profile_dir` lists, for removing their tokens when the folder is deleted.
    /// Unreadable or missing lists give none.
    pub fn stored_ids(profile_dir: &Path) -> Vec<String> {
        match read_json::<Record>(&profile_dir.join(FILE)) {
            JsonFile::Valid(record) => record
                .agents
                .into_values()
                .flat_map(|agent| agent.accounts.into_iter().map(|account| account.id))
                .collect(),
            _ => Vec::new(),
        }
    }

    pub fn list(&self, agent: &str) -> Result<AgentAccounts, AccountError> {
        let _guard = self.guard();
        let record = self.read()?;
        Ok(self.view(agent, &record))
    }

    /// Adds an account whose token `claude setup-token` printed. The token is stored first, so a
    /// listed account always has one. The account is not used until it is chosen.
    pub fn add(
        &self,
        agent: &str,
        name: &str,
        token: &str,
        now: Millis,
    ) -> Result<AgentAccounts, AccountError> {
        let vault = self.usable(agent)?;
        let name = check_name(name)?;
        let token = check_token(token)?;
        let _guard = self.guard();
        let mut record = self.read()?;
        let entry = record.agents.entry(agent.to_owned()).or_default();
        if entry
            .accounts
            .iter()
            .any(|account| account.name.to_lowercase() == name.to_lowercase())
        {
            return Err(AccountError::Invalid(format!(
                "An account named {name} is already added. Choose another name."
            )));
        }
        if entry.accounts.len() >= MAX_ACCOUNTS {
            return Err(AccountError::Invalid(format!(
                "Remove an account first: a profile keeps at most {MAX_ACCOUNTS} per agent."
            )));
        }
        let id = crate::ids::new_id();
        vault.save(&id, &token).map_err(AccountError::Vault)?;
        entry.accounts.push(AgentAccount {
            id: id.clone(),
            name,
            added_at: now,
        });
        if let Err(error) = self.write(&record) {
            let _ = vault.remove(&id);
            return Err(error);
        }
        Ok(self.view(agent, &record))
    }

    /// Removes an account and its token. Chats running on it keep their process until it stops;
    /// their next start uses the agent's own sign-in, unless another account is chosen.
    pub fn remove(&self, agent: &str, account: &str) -> Result<AgentAccounts, AccountError> {
        let vault = self.usable(agent)?;
        let _guard = self.guard();
        let mut record = self.read()?;
        let entry = record.agents.entry(agent.to_owned()).or_default();
        let Some(at) = entry.accounts.iter().position(|item| item.id == account) else {
            return Err(AccountError::Unknown);
        };
        // The token goes first: a failure keeps the listed account, which can be removed again.
        vault.remove(account).map_err(AccountError::Vault)?;
        entry.accounts.remove(at);
        if entry.active.as_deref() == Some(account) {
            entry.active = None;
        }
        self.write(&record)?;
        Ok(self.view(agent, &record))
    }

    /// Chooses the account agents start on from now. `None`: the agent's own sign-in.
    pub fn activate(
        &self,
        agent: &str,
        account: Option<&str>,
    ) -> Result<AgentAccounts, AccountError> {
        if account.is_some() {
            self.usable(agent)?;
        } else if !SUPPORTED.contains(&agent) {
            return Err(unsupported(agent));
        }
        let _guard = self.guard();
        let mut record = self.read()?;
        let entry = record.agents.entry(agent.to_owned()).or_default();
        if let Some(account) = account
            && !entry.accounts.iter().any(|item| item.id == account)
        {
            return Err(AccountError::Unknown);
        }
        if entry.active.as_deref() != account {
            entry.active = account.map(str::to_owned);
            self.write(&record)?;
        }
        Ok(self.view(agent, &record))
    }

    /// The account a start of `agent` runs on, with its token; `None` for the agent's own sign-in.
    /// A chosen account whose token cannot be read is an error, never a quiet fallback to another.
    pub fn launch(&self, agent: &str) -> Result<Option<AccountLaunch>, AccountError> {
        if !SUPPORTED.contains(&agent) {
            return Ok(None);
        }
        let record = {
            let _guard = self.guard();
            self.read()?
        };
        let Some(entry) = record.agents.get(agent) else {
            return Ok(None);
        };
        let Some(account) = entry
            .active
            .as_deref()
            .and_then(|id| entry.accounts.iter().find(|item| item.id == id))
        else {
            return Ok(None);
        };
        let vault = self.vault.as_ref().map_err(|reason| {
            AccountError::Unavailable(format!(
                "{reason} Switch {} to its own sign-in to continue.",
                account.name
            ))
        })?;
        match vault.load(&account.id) {
            Ok(Some(token)) => Ok(Some(AccountLaunch {
                account_id: account.id.clone(),
                name: account.name.clone(),
                token,
            })),
            Ok(None) => Err(AccountError::Vault(format!(
                "The {} account's token is missing from this computer's secure storage. Add the account again, or switch accounts.",
                account.name
            ))),
            Err(error) => Err(AccountError::Vault(error)),
        }
    }

    /// An account's name, when `agent` lists it.
    pub fn name(&self, agent: &str, account: &str) -> Option<String> {
        let _guard = self.guard();
        self.read()
            .ok()?
            .agents
            .get(agent)?
            .accounts
            .iter()
            .find(|item| item.id == account)
            .map(|item| item.name.clone())
    }

    fn usable(&self, agent: &str) -> Result<&Arc<dyn TokenVault>, AccountError> {
        if !SUPPORTED.contains(&agent) {
            return Err(unsupported(agent));
        }
        self.vault
            .as_ref()
            .map_err(|reason| AccountError::Unavailable(reason.clone()))
    }

    fn view(&self, agent: &str, record: &Record) -> AgentAccounts {
        let entry = record.agents.get(agent).cloned().unwrap_or_default();
        let active = entry
            .active
            .filter(|id| entry.accounts.iter().any(|item| &item.id == id));
        AgentAccounts {
            agent_id: agent.to_owned(),
            accounts: entry.accounts,
            active,
            unavailable: if SUPPORTED.contains(&agent) {
                self.vault.as_ref().err().cloned()
            } else {
                Some(unsupported(agent).to_string())
            },
        }
    }

    /// The saved record. A damaged file is set aside and the list starts empty; its tokens stay in
    /// secure storage. A file from a newer Ghost is refused, so it is never overwritten.
    fn read(&self) -> Result<Record, AccountError> {
        match read_json::<serde_json::Value>(&self.path) {
            JsonFile::Missing => Ok(Record {
                version: VERSION,
                ..Record::default()
            }),
            JsonFile::Valid(value) => {
                if value
                    .get("version")
                    .and_then(serde_json::Value::as_u64)
                    .is_some_and(|version| version > u64::from(VERSION))
                {
                    return Err(AccountError::Unavailable(
                        "A newer Ghost saved these accounts. Update Ghost to use them.".into(),
                    ));
                }
                match serde_json::from_value::<Record>(value) {
                    Ok(record) if record.version == VERSION => Ok(record),
                    _ => self.set_aside(),
                }
            }
            JsonFile::Unusable(_) => self.set_aside(),
            JsonFile::Unreadable(error) => Err(AccountError::Storage(error.to_string())),
        }
    }

    fn set_aside(&self) -> Result<Record, AccountError> {
        let moved = quarantine_file(&self.path)
            .map_err(|error| AccountError::Storage(error.to_string()))?;
        tracing::warn!(path = %moved.display(), "set aside an unreadable agent account list");
        Ok(Record {
            version: VERSION,
            ..Record::default()
        })
    }

    fn write(&self, record: &Record) -> Result<(), AccountError> {
        write_json_atomic(&self.path, record)
            .map_err(|error| AccountError::Storage(error.to_string()))
    }

    fn guard(&self) -> MutexGuard<'_, ()> {
        match self.lock.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

fn unsupported(agent: &str) -> AccountError {
    AccountError::Unavailable(match agent {
        "codex" => "Codex keeps each sign-in's chats in that sign-in's own folder, so another Codex account could not continue a chat. Hand the chat off to another agent instead.".into(),
        _ => "Only Claude Code takes added accounts for now.".into(),
    })
}

fn check_name(name: &str) -> Result<String, AccountError> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        return Err(AccountError::Invalid(
            "Name the account, such as Work.".into(),
        ));
    }
    if name.chars().count() > MAX_NAME_CHARS || name.chars().any(char::is_control) {
        return Err(AccountError::Invalid(format!(
            "Use a name of at most {MAX_NAME_CHARS} characters."
        )));
    }
    Ok(name)
}

/// The token without the line breaks and spaces a copy from a terminal can carry.
fn check_token(token: &str) -> Result<String, AccountError> {
    let token: String = token.chars().filter(|c| !c.is_whitespace()).collect();
    let shaped = token.starts_with(TOKEN_PREFIX)
        && TOKEN_CHARS.contains(&token.len())
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if shaped {
        return Ok(token);
    }
    Err(AccountError::Invalid(if token.starts_with("sk-ant-api") {
        "That is an API key. Paste the token claude setup-token printed, which starts with sk-ant-oat.".into()
    } else {
        "Paste the whole token claude setup-token printed. It starts with sk-ant-oat.".into()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct MemoryVault {
        tokens: Mutex<HashMap<String, String>>,
        fail: Mutex<bool>,
    }

    impl TokenVault for MemoryVault {
        fn load(&self, account: &str) -> Result<Option<String>, String> {
            Ok(self.tokens.lock().unwrap().get(account).cloned())
        }
        fn save(&self, account: &str, token: &str) -> Result<(), String> {
            if *self.fail.lock().unwrap() {
                return Err("Secure storage is locked.".into());
            }
            self.tokens
                .lock()
                .unwrap()
                .insert(account.into(), token.into());
            Ok(())
        }
        fn remove(&self, account: &str) -> Result<(), String> {
            if *self.fail.lock().unwrap() {
                return Err("Secure storage is locked.".into());
            }
            self.tokens.lock().unwrap().remove(account);
            Ok(())
        }
    }

    const TOKEN: &str = "sk-ant-oat01-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789-AA";

    fn store(dir: &Path) -> (AgentAccountsStore, Arc<MemoryVault>) {
        let vault = Arc::new(MemoryVault::default());
        let shared: Arc<dyn TokenVault> = vault.clone();
        (AgentAccountsStore::open(dir, Ok(shared)), vault)
    }

    #[test]
    fn an_added_account_keeps_its_token_in_the_vault_and_runs_once_chosen() {
        let dir = tempfile::tempdir().unwrap();
        let (accounts, vault) = store(dir.path());
        assert_eq!(
            accounts.launch("claude").unwrap(),
            None,
            "own sign-in at first"
        );
        // A copy from a terminal can break the token over lines.
        let split = format!("  {}\n{}  ", &TOKEN[..50], &TOKEN[50..]);
        let added = accounts
            .add("claude", "  Work   laptop ", &split, 7)
            .unwrap();
        let work = &added.accounts[0];
        assert_eq!(work.name, "Work laptop");
        assert_eq!(work.added_at, 7);
        assert_eq!(added.active, None, "adding does not switch");
        assert_eq!(vault.tokens.lock().unwrap()[&work.id], TOKEN);
        let file = std::fs::read_to_string(dir.path().join(FILE)).unwrap();
        assert!(
            !file.contains("sk-ant"),
            "the file names accounts only: {file}"
        );
        assert_eq!(accounts.launch("claude").unwrap(), None);

        let chosen = accounts.activate("claude", Some(&work.id)).unwrap();
        assert_eq!(chosen.active.as_deref(), Some(work.id.as_str()));
        let launch = accounts.launch("claude").unwrap().unwrap();
        assert_eq!(launch.account_id, work.id);
        assert_eq!(launch.name, "Work laptop");
        assert_eq!(launch.token, TOKEN);
        assert!(
            !format!("{launch:?}").contains("sk-ant"),
            "Debug never prints the token"
        );

        // Reopening reads the same choice.
        let shared: Arc<dyn TokenVault> = vault.clone();
        let again = AgentAccountsStore::open(dir.path(), Ok(shared));
        assert_eq!(
            again.list("claude").unwrap().active.as_deref(),
            Some(work.id.as_str())
        );
        assert_eq!(
            again.name("claude", &work.id).as_deref(),
            Some("Work laptop")
        );
        assert_eq!(
            AgentAccountsStore::stored_ids(dir.path()),
            vec![work.id.clone()]
        );

        assert_eq!(accounts.activate("claude", None).unwrap().active, None);
        assert_eq!(accounts.launch("claude").unwrap(), None);
    }

    #[test]
    fn removing_an_account_removes_its_token_and_its_choice() {
        let dir = tempfile::tempdir().unwrap();
        let (accounts, vault) = store(dir.path());
        let id = accounts.add("claude", "Work", TOKEN, 1).unwrap().accounts[0]
            .id
            .clone();
        accounts.activate("claude", Some(&id)).unwrap();
        *vault.fail.lock().unwrap() = true;
        assert!(matches!(
            accounts.remove("claude", &id),
            Err(AccountError::Vault(_))
        ));
        assert_eq!(
            accounts.list("claude").unwrap().accounts.len(),
            1,
            "kept to retry"
        );
        *vault.fail.lock().unwrap() = false;
        let left = accounts.remove("claude", &id).unwrap();
        assert!(left.accounts.is_empty());
        assert_eq!(left.active, None);
        assert!(vault.tokens.lock().unwrap().is_empty());
        assert_eq!(accounts.remove("claude", &id), Err(AccountError::Unknown));
        assert_eq!(
            accounts.activate("claude", Some(&id)),
            Err(AccountError::Unknown)
        );
    }

    #[test]
    fn names_tokens_and_agents_are_checked() {
        let dir = tempfile::tempdir().unwrap();
        let (accounts, vault) = store(dir.path());
        for (name, token) in [
            ("", TOKEN),
            (&"x".repeat(41), TOKEN),
            ("Work", "sk-ant-oat01-short"),
            (
                "Work",
                "sk-ant-api03-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789AbCdEfGhIjKlMnOp",
            ),
            ("Work", &format!("{TOKEN}!")),
        ] {
            assert!(
                matches!(
                    accounts.add("claude", name, token, 1),
                    Err(AccountError::Invalid(_))
                ),
                "{name:?} {token:?}"
            );
        }
        let api = accounts
            .add(
                "claude",
                "Work",
                "sk-ant-api03-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789AbCdEfGhIjKlMnOp",
                1,
            )
            .unwrap_err()
            .to_string();
        assert!(api.contains("API key"), "{api}");
        accounts.add("claude", "Work", TOKEN, 1).unwrap();
        assert!(matches!(
            accounts.add("claude", "work", TOKEN, 2),
            Err(AccountError::Invalid(_))
        ));
        for agent in ["codex", "cursor"] {
            assert!(matches!(
                accounts.add(agent, "Work", TOKEN, 1),
                Err(AccountError::Unavailable(_))
            ));
            assert!(accounts.list(agent).unwrap().unavailable.is_some());
            assert_eq!(accounts.launch(agent).unwrap(), None);
        }
        assert!(
            accounts
                .list("codex")
                .unwrap()
                .unavailable
                .unwrap()
                .contains("own folder")
        );
        assert_eq!(vault.tokens.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_chosen_account_without_its_token_refuses_to_start_on_another() {
        let dir = tempfile::tempdir().unwrap();
        let (accounts, vault) = store(dir.path());
        let id = accounts.add("claude", "Work", TOKEN, 1).unwrap().accounts[0]
            .id
            .clone();
        accounts.activate("claude", Some(&id)).unwrap();
        vault.tokens.lock().unwrap().clear();
        let error = accounts.launch("claude").unwrap_err().to_string();
        assert!(
            error.contains("Work") && error.contains("missing"),
            "{error}"
        );

        let unavailable =
            AgentAccountsStore::open(dir.path(), Err("No secure storage here.".into()));
        assert!(matches!(
            unavailable.launch("claude"),
            Err(AccountError::Unavailable(_))
        ));
        assert_eq!(
            unavailable.list("claude").unwrap().unavailable.as_deref(),
            Some("No secure storage here.")
        );
        assert!(matches!(
            unavailable.add("claude", "Home", TOKEN, 1),
            Err(AccountError::Unavailable(_))
        ));
    }

    #[test]
    fn a_damaged_list_is_set_aside_and_a_newer_one_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let (accounts, _) = store(dir.path());
        std::fs::write(dir.path().join(FILE), b"{not json").unwrap();
        assert!(accounts.list("claude").unwrap().accounts.is_empty());
        assert!(
            std::fs::read_dir(dir.path()).unwrap().any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("agent-accounts.json.corrupt-")),
            "the damaged file is preserved"
        );
        std::fs::write(dir.path().join(FILE), br#"{"version":2,"agents":{}}"#).unwrap();
        assert!(matches!(
            accounts.add("claude", "Work", TOKEN, 1),
            Err(AccountError::Unavailable(_))
        ));
        assert_eq!(
            std::fs::read_to_string(dir.path().join(FILE)).unwrap(),
            r#"{"version":2,"agents":{}}"#
        );
    }
}
