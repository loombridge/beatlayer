//! The agent server's settings, status and tokens.

use super::Shell;
use crate::agent;
use crate::agent::AgentServerStatus;
use crate::agent::AgentSettings;
use bl_assistant::keychain::Keychain;
use std::sync::Arc;

use super::{ShellError, to_value};
use crate::agent::AgentServerState;
use crate::agent::AutonomousArgs;
use crate::agent::ClientArgs;
use crate::agent::SetServerArgs;
use bl_commands::NoArgs;
use serde_json::Value;

impl Shell {
    /// Makes each allowed agent that has no token of its own one (an agent
    /// allowed before agents had them, or one whose token the keychain
    /// lost), and the server one while it's on (WP-1.15b). The caller keeps
    /// them.
    pub(super) fn give_agents_tokens(&mut self) {
        for client in &mut self.prefs.agent.clients {
            if client.token.is_empty()
                && let Ok(token) = agent::new_token()
            {
                client.token = token.into();
            }
        }
        if self.prefs.agent.server
            && self.prefs.agent.token.is_empty()
            && let Ok(token) = agent::new_token()
        {
            self.prefs.agent.token = token.into();
        }
    }

    /// Keeps the agent server's tokens in `keychain` from now on: the OS
    /// keychain in the app, an in-memory one in tests. Reads the tokens it
    /// holds, moves any that the preferences' file still had into it and
    /// rewrites the file without them, and makes the ones still missing.
    ///
    /// If the keychain can't be read or written, the Status Bar says so and
    /// the tokens last only until Beatlayer quits; the agents keep being
    /// allowed, and are given new tokens at the next start.
    pub fn use_agent_keychain(&mut self, keychain: Arc<dyn Keychain>) {
        let stored = match keychain.read(agent::KEYCHAIN_ACCOUNT) {
            Ok(Some(text)) => agent::StoredTokens::parse(&text),
            Ok(None) => Ok(agent::StoredTokens::default()),
            Err(error) => Err(error.to_string()),
        };
        self.agent_keychain = Some(keychain);
        match stored {
            Ok(stored) => {
                stored.apply(&mut self.prefs.agent);
                self.agent_tokens_stored = Some(stored);
            }
            Err(error) => {
                self.agent_tokens_stored = None;
                self.set_message(
                    format!("Couldn't read the agent tokens from the keychain: {error}"),
                    true,
                );
            }
        }
        self.give_agents_tokens();
        if self.store_agent_tokens() && self.agent_tokens_in_file {
            // Only once the keychain has them: the file loses its copy.
            self.agent_tokens_in_file = false;
            self.save_preferences();
        }
    }

    /// Writes the agent server's tokens to the keychain if they changed
    /// since it was last read or written. Returns whether the keychain now
    /// holds them; a failure shows in the Status Bar.
    pub(super) fn store_agent_tokens(&mut self) -> bool {
        let Some(keychain) = &self.agent_keychain else {
            return false;
        };
        let tokens = agent::StoredTokens::of(&self.prefs.agent);
        if self.agent_tokens_stored.as_ref() == Some(&tokens) {
            return true;
        }
        // No tokens, and a keychain that couldn't be read: nothing to keep,
        // and nothing worth a message about.
        if tokens.is_empty() && self.agent_tokens_stored.is_none() {
            return true;
        }
        let written = if tokens.is_empty() {
            keychain.delete(agent::KEYCHAIN_ACCOUNT)
        } else {
            keychain.write(agent::KEYCHAIN_ACCOUNT, &tokens.to_text())
        };
        match written {
            Ok(()) => {
                self.agent_tokens_stored = Some(tokens);
                true
            }
            Err(error) => {
                self.set_message(
                    format!(
                        "Couldn't keep the agent tokens in the keychain: {error}. They last \
                         until Beatlayer quits."
                    ),
                    true,
                );
                false
            }
        }
    }

    /// The agent server's settings.
    #[must_use]
    pub fn agent_settings(&self) -> &AgentSettings {
        &self.prefs.agent
    }

    /// What the app last said about the running agent server.
    #[must_use]
    pub fn agent_status(&self) -> &AgentServerStatus {
        &self.agent_status
    }

    /// Records what the app says about the running agent server, for the
    /// Settings window.
    pub fn set_agent_status(&mut self, status: AgentServerStatus) {
        if self.agent_status != status {
            self.agent_status = status;
            self.touch();
        }
    }

    /// Runs the `agent` commands.
    pub(super) fn run_agent(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let failed = |message: String| ShellError::Failed {
            command: id.to_owned(),
            message,
        };
        let unavailable = |reason: &str| ShellError::Unavailable {
            command: id.to_owned(),
            reason: reason.to_owned(),
        };
        match id {
            agent::AGENT_SET_SERVER => {
                let SetServerArgs { on, port } = Self::args(id, args)?;
                if let Some(port) = port {
                    if port < 1024 {
                        return Err(ShellError::InvalidArgs {
                            command: id.to_owned(),
                            message: format!("port {port} is below 1024"),
                        });
                    }
                    self.prefs.agent.port = port;
                }
                if let Some(on) = on {
                    self.prefs.agent.server = on;
                }
                if self.prefs.agent.server && self.prefs.agent.token.is_empty() {
                    self.prefs.agent.token = agent::new_token().map_err(failed)?.into();
                }
                self.save_preferences();
                Ok(to_value(&AgentServerState::from(&self.prefs.agent)))
            }
            agent::AGENT_APPROVE_CLIENT => {
                let ClientArgs { client } = Self::args(id, args)?;
                if client.trim().is_empty() {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: "the agent's name is empty".to_owned(),
                    });
                }
                if self.prefs.agent.client(&client).is_none() {
                    // Its rights go with a token of its own (WP-1.15b).
                    let token = agent::new_token().map_err(failed)?;
                    self.prefs.agent.clients.push(agent::AgentClient {
                        name: client,
                        autonomous: false,
                        token: token.into(),
                    });
                    self.save_preferences();
                }
                Ok(to_value(&AgentServerState::from(&self.prefs.agent)))
            }
            agent::AGENT_FORGET_CLIENT => {
                let ClientArgs { client } = Self::args(id, args)?;
                let before = self.prefs.agent.clients.len();
                self.prefs.agent.clients.retain(|c| c.name != client);
                if self.prefs.agent.clients.len() == before {
                    return Err(unavailable(&format!(
                        "no agent called “{client}” is allowed"
                    )));
                }
                self.save_preferences();
                Ok(to_value(&AgentServerState::from(&self.prefs.agent)))
            }
            agent::AGENT_SET_AUTONOMOUS => {
                let AutonomousArgs { client, on } = Self::args(id, args)?;
                let Some(entry) = self
                    .prefs
                    .agent
                    .clients
                    .iter_mut()
                    .find(|c| c.name == client)
                else {
                    return Err(unavailable(&format!(
                        "no agent called “{client}” is allowed; it's allowed when it first \
                         connects"
                    )));
                };
                entry.autonomous = on;
                self.save_preferences();
                Ok(to_value(&AgentServerState::from(&self.prefs.agent)))
            }
            agent::AGENT_NEW_TOKEN => {
                Self::args::<NoArgs>(id, args)?;
                self.prefs.agent.token = agent::new_token().map_err(failed)?.into();
                self.save_preferences();
                Ok(to_value(&AgentServerState::from(&self.prefs.agent)))
            }
            agent::AGENT_NEW_CLIENT_TOKEN => {
                let ClientArgs { client } = Self::args(id, args)?;
                let token = agent::new_token().map_err(failed)?;
                let Some(entry) = self
                    .prefs
                    .agent
                    .clients
                    .iter_mut()
                    .find(|c| c.name == client)
                else {
                    return Err(unavailable(&format!(
                        "no agent called “{client}” is allowed"
                    )));
                };
                entry.token = token.into();
                self.save_preferences();
                Ok(to_value(&AgentServerState::from(&self.prefs.agent)))
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }
}
