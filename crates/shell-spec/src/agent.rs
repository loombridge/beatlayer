//! The agent server's settings and commands (WP-1.15, decision D17): the
//! switch that serves the agent API over Streamable HTTP from the app, the
//! agents a person has allowed, and which of them may run destructive and
//! external commands without asking.
//!
//! The settings live in the preferences and show in the Settings window,
//! on the Tempo & MIDI page beside Live's control surfaces, which are the
//! other way another program drives the app. They change what agents may
//! do, so only a person changes them: the agent API refuses every command
//! in this module, whatever the agent's permissions.

use bl_commands::Context::Settings;
use bl_commands::{CommandSpec, Permission, RegisterError, Registry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `agent.set_server`: switch the agent server on or off, or move it.
pub const AGENT_SET_SERVER: &str = "agent.set_server";
/// `agent.approve_client`: allow an agent to connect.
pub const AGENT_APPROVE_CLIENT: &str = "agent.approve_client";
/// `agent.forget_client`: withdraw an agent's approval.
pub const AGENT_FORGET_CLIENT: &str = "agent.forget_client";
/// `agent.set_autonomous`: let an agent run destructive and external
/// commands without asking.
pub const AGENT_SET_AUTONOMOUS: &str = "agent.set_autonomous";
/// `agent.new_token`: replace the server's access token.
pub const AGENT_NEW_TOKEN: &str = "agent.new_token";

/// Every command [`register`] adds.
pub const IDS: &[&str] = &[
    AGENT_SET_SERVER,
    AGENT_APPROVE_CLIENT,
    AGENT_FORGET_CLIENT,
    AGENT_SET_AUTONOMOUS,
    AGENT_NEW_TOKEN,
];

/// Whether `id` is one of the agent server's commands, which agents may
/// never run.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// The port the agent server listens on unless the person picks another.
pub const DEFAULT_PORT: u16 = 37_420;

/// The path the agent server serves the agent API at.
pub const ENDPOINT_PATH: &str = "/mcp";

/// The agent server's settings, kept in the preferences.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct AgentSettings {
    /// Whether the app serves the agent API on `127.0.0.1`. Off until the
    /// person switches it on.
    pub server: bool,
    /// The port it listens on.
    pub port: u16,
    /// The bearer token every request must carry. Empty until the server is
    /// first switched on.
    #[schemars(skip)]
    pub token: String,
    /// The agents the person has allowed to connect, in the order they were
    /// allowed.
    pub clients: Vec<AgentClient>,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            server: false,
            port: DEFAULT_PORT,
            token: String::new(),
            clients: Vec::new(),
        }
    }
}

impl AgentSettings {
    /// The allowed agent called `name`, if there is one.
    #[must_use]
    pub fn client(&self, name: &str) -> Option<&AgentClient> {
        self.clients.iter().find(|client| client.name == name)
    }

    /// Whether the agent called `name` may run destructive and external
    /// commands without asking.
    #[must_use]
    pub fn is_autonomous(&self, name: &str) -> bool {
        self.client(name).is_some_and(|client| client.autonomous)
    }

    /// The address agents connect to.
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}{ENDPOINT_PATH}", self.port)
    }
}

/// An agent the person has allowed to connect.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentClient {
    /// The name the agent gives when it connects, such as `claude-code`.
    pub name: String,
    /// Whether it runs destructive and external commands without asking
    /// (autonomous mode).
    #[serde(default)]
    pub autonomous: bool,
}

/// Arguments of `agent.set_server`: what to change. Fields left out keep
/// their values.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetServerArgs {
    /// Whether the server runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
    /// The port to listen on, from 1024 to 65535.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1024))]
    pub port: Option<u16>,
}

/// Arguments of `agent.approve_client` and `agent.forget_client`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClientArgs {
    /// The agent's name, as it gives it when it connects.
    pub client: String,
}

/// Arguments of `agent.set_autonomous`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AutonomousArgs {
    /// The agent's name, as it gives it when it connects.
    pub client: String,
    /// Whether it runs destructive and external commands without asking.
    pub on: bool,
}

/// What the agent server's commands return: its settings afterwards,
/// without the token, which shows only in the Settings window.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentServerState {
    /// Whether the server is switched on.
    pub server: bool,
    /// The address agents connect to.
    pub url: String,
    /// The agents allowed to connect.
    pub clients: Vec<AgentClient>,
}

impl From<&AgentSettings> for AgentServerState {
    fn from(settings: &AgentSettings) -> Self {
        Self {
            server: settings.server,
            url: settings.url(),
            clients: settings.clients.clone(),
        }
    }
}

/// The sentence every agent command's description ends with.
macro_rules! person_only {
    ($text:literal) => {
        concat!(
            $text,
            " It changes what agents may do, so only a person can run it, in the Settings \
             window: the agent API refuses it."
        )
    };
}

/// Adds the agent server's commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    let specs = [
        CommandSpec::new(AGENT_SET_SERVER, "Agent Server")
            .description(person_only!(
                "Switches the agent server on or off, or moves it to another port. While it's \
                 on, agent clients on this computer reach the agent API over Streamable HTTP at \
                 `http://127.0.0.1:<port>/mcp`, each request carrying the server's token as a \
                 bearer token; requests from web pages are refused. Switching it on the first \
                 time makes the token. The setting is kept for the next start."
            ))
            .permission(Permission::External)
            .context(Settings)
            .args::<SetServerArgs>()
            .result::<AgentServerState>(),
        CommandSpec::new(AGENT_APPROVE_CLIENT, "Allow Agent")
            .description(person_only!(
                "Allows the agent called `client` to connect to the agent server, as answering \
                 Allow when it first connects does. An agent is asked about once; its approval \
                 is kept for the next start."
            ))
            .permission(Permission::External)
            .context(Settings)
            .args::<ClientArgs>()
            .result::<AgentServerState>(),
        CommandSpec::new(AGENT_FORGET_CLIENT, "Forget Agent")
            .description(person_only!(
                "Withdraws the approval of the agent called `client`, so the person is asked \
                 again the next time it connects. Its sessions end."
            ))
            .permission(Permission::Edit)
            .context(Settings)
            .args::<ClientArgs>()
            .result::<AgentServerState>(),
        CommandSpec::new(AGENT_SET_AUTONOMOUS, "Autonomous")
            .description(person_only!(
                "Switches autonomous mode for the allowed agent called `client`. With it on, the \
                 agent runs destructive commands (deleting, discarding, overwriting) and \
                 external ones (files outside the project, devices, other applications) \
                 without asking the person first; with it off, as it starts, each one waits for \
                 the person's confirmation."
            ))
            .permission(Permission::External)
            .context(Settings)
            .args::<AutonomousArgs>()
            .result::<AgentServerState>(),
        CommandSpec::new(AGENT_NEW_TOKEN, "New Token")
            .description(person_only!(
                "Replaces the agent server's token with a new random one, so agents configured \
                 with the old token can no longer connect until they're given the new one, \
                 which the Settings window shows. Use it if the token may have leaked."
            ))
            .permission(Permission::External)
            .context(Settings)
            .args::<bl_commands::NoArgs>()
            .result::<AgentServerState>(),
    ];
    for spec in specs {
        registry.register(spec)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_agent_command_is_registered_and_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), IDS.len());
        for id in IDS {
            assert!(registry.contains(id), "{id}");
            assert!(handles(id));
        }
        assert!(registry.problems().is_empty(), "{:?}", registry.problems());
        assert_eq!(
            registry.get(AGENT_SET_SERVER).unwrap().permission,
            Some(Permission::External)
        );
        assert_eq!(
            registry.get(AGENT_SET_AUTONOMOUS).unwrap().permission,
            Some(Permission::External)
        );
    }

    #[test]
    fn settings_default_to_off_on_the_default_port() {
        let settings = AgentSettings::default();
        assert!(!settings.server);
        assert_eq!(settings.url(), "http://127.0.0.1:37420/mcp");
        let read: AgentSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(read, settings);
        let state = AgentServerState::from(&settings);
        assert!(!serde_json::to_string(&state).unwrap().contains("token"));
    }
}
