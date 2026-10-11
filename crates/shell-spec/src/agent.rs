//! The agent server's settings and commands (WP-1.15, decision D17): the
//! switch that serves the agent API over Streamable HTTP from the app, the
//! agents a person has allowed, and which of them may run destructive and
//! external commands without asking.
//!
//! An allowed agent's rights belong to its own token, which Beatlayer makes
//! when the person allows it (WP-1.15b), not to the name a client gives
//! when it connects: a client that connects with the server's token is
//! asked about each time and is never autonomous, whatever name it gives.
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
/// `agent.new_client_token`: replace an allowed agent's own token.
pub const AGENT_NEW_CLIENT_TOKEN: &str = "agent.new_client_token";

/// Every command [`register`] adds.
pub const IDS: &[&str] = &[
    AGENT_SET_SERVER,
    AGENT_APPROVE_CLIENT,
    AGENT_FORGET_CLIENT,
    AGENT_SET_AUTONOMOUS,
    AGENT_NEW_TOKEN,
    AGENT_NEW_CLIENT_TOKEN,
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

/// A token: a secret kept in the preferences that never shows in a debug
/// print, a log, or a command's result. It's compared in constant time
/// where requests are checked (the agent API's HTTP server).
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    /// The token's text, to put in the preferences' file, show to the
    /// person, or check a request against.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl From<String> for Secret {
    fn from(text: String) -> Self {
        Self(text)
    }
}

impl std::ops::Deref for Secret {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_empty() {
            "Secret(none)"
        } else {
            "Secret(…)"
        })
    }
}

impl PartialEq<str> for Secret {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<String> for Secret {
    fn eq(&self, other: &String) -> bool {
        self.0 == *other
    }
}

impl PartialEq<Secret> for String {
    fn eq(&self, other: &Secret) -> bool {
        *self == other.0
    }
}

/// The agent server's settings, kept in the preferences.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct AgentSettings {
    /// Whether the app serves the agent API on `127.0.0.1`. Off until the
    /// person switches it on.
    pub server: bool,
    /// The port it listens on.
    pub port: u16,
    /// The server's token: the bearer token an agent connects with before
    /// the person has allowed it, or while it hasn't been given its own.
    /// Empty until the server is first switched on.
    #[schemars(skip)]
    pub token: Secret,
    /// The agents the person has allowed to connect, in the order they were
    /// allowed.
    pub clients: Vec<AgentClient>,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            server: false,
            port: DEFAULT_PORT,
            token: Secret::default(),
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
    /// commands without asking. Only a request carrying that agent's own
    /// token speaks for it.
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
    /// The name the agent gave when the person allowed it, such as
    /// `claude-code`.
    pub name: String,
    /// Whether it runs destructive and external commands without asking
    /// (autonomous mode).
    #[serde(default)]
    pub autonomous: bool,
    /// Its own token, made when the person allowed it: a request carrying
    /// it is this agent, whatever name its client gives, and only such a
    /// request has this agent's rights (connecting without being asked, and
    /// autonomous mode).
    #[serde(default)]
    #[schemars(skip)]
    pub token: Secret,
}

/// An allowed agent as the agent server's commands report it: without its
/// token.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AllowedAgent {
    /// Its name.
    pub name: String,
    /// Whether it runs destructive and external commands without asking.
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
    pub clients: Vec<AllowedAgent>,
}

impl From<&AgentSettings> for AgentServerState {
    fn from(settings: &AgentSettings) -> Self {
        Self {
            server: settings.server,
            url: settings.url(),
            clients: settings
                .clients
                .iter()
                .map(|client| AllowedAgent {
                    name: client.name.clone(),
                    autonomous: client.autonomous,
                })
                .collect(),
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
                 `http://127.0.0.1:<port>/mcp`, each request carrying a token as a bearer \
                 token: an allowed agent's own token, or the server's token, with which a client \
                 is asked about each time it connects. Requests from web pages are refused. \
                 Switching it on the first time makes the server's token. The setting is kept \
                 for the next start."
            ))
            .permission(Permission::External)
            .context(Settings)
            .args::<SetServerArgs>()
            .result::<AgentServerState>(),
        CommandSpec::new(AGENT_APPROVE_CLIENT, "Allow Agent")
            .description(person_only!(
                "Allows the agent called `client` to connect to the agent server, as answering \
                 Allow when it first connects does, and makes it its own token. Its approval \
                 and its autonomous mode belong to that token, not to its name: a client that \
                 connects with the agent's token isn't asked about again, and one that connects \
                 with the server's token is asked each time and is never autonomous, whatever \
                 name it gives. The approval is kept for the next start."
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
                 agent runs destructive commands (deleting, discarding, overwriting, or placing \
                 clips over others) and external ones (files outside the project, devices, \
                 other applications) without asking the person first; with it off, as it \
                 starts, each one waits for the person's confirmation. It applies only to \
                 requests that carry the agent's own token."
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
        CommandSpec::new(AGENT_NEW_CLIENT_TOKEN, "New Agent Token")
            .description(person_only!(
                "Replaces the own token of the allowed agent called `client` with a new random \
                 one, so a client configured with the old one can no longer speak for that \
                 agent until it's given the new one: requests carrying the old one are refused. \
                 Use it if the agent's token may have leaked."
            ))
            .permission(Permission::External)
            .context(Settings)
            .args::<ClientArgs>()
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

    #[test]
    fn tokens_never_show_in_results_or_debug_prints() {
        let settings = AgentSettings {
            token: Secret::from("a".repeat(64)),
            clients: vec![AgentClient {
                name: "claude-code".to_owned(),
                autonomous: true,
                token: Secret::from("b".repeat(64)),
            }],
            ..AgentSettings::default()
        };
        let state = serde_json::to_string(&AgentServerState::from(&settings)).unwrap();
        assert!(
            !state.contains("aaaa") && !state.contains("bbbb"),
            "{state}"
        );
        let printed = format!("{settings:?}");
        assert!(
            !printed.contains("aaaa") && !printed.contains("bbbb"),
            "{printed}"
        );
        // The preferences keep them, and read them back.
        let saved = serde_json::to_string(&settings).unwrap();
        let read: AgentSettings = serde_json::from_str(&saved).unwrap();
        assert_eq!(read, settings);
        assert_eq!(read.clients[0].token, "b".repeat(64));
        // Preferences from before agents had tokens still read.
        let old: AgentSettings =
            serde_json::from_str(r#"{"clients":[{"name":"x","autonomous":true}]}"#).unwrap();
        assert!(old.clients[0].token.is_empty());
    }
}
