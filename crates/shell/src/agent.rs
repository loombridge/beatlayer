//! The agent server's settings at work (WP-1.15): its token, where the
//! tokens are kept, and what the app reports about the running server for
//! the Settings window.
//!
//! The server's token and each allowed agent's live in the OS keychain, in
//! one entry ([`KEYCHAIN_SERVICE`], [`KEYCHAIN_ACCOUNT`]) holding them all,
//! and never in the preferences' file.
//! [`Shell::use_agent_keychain`](crate::Shell::use_agent_keychain) reads
//! them at start-up and moves any that an older preferences file kept
//! there.
//!
//! The commands that change the settings are in [`bl_shell_spec::agent`];
//! [`Shell::run`](crate::Shell::run) runs them. The server itself is the
//! agent API's (`bl-agent`), started and stopped by the app as the settings
//! say.

use std::collections::BTreeMap;
use std::sync::Arc;

use bl_assistant::keychain::{Keychain, OsKeychain};

use serde::{Deserialize, Serialize};

pub use bl_shell_spec::agent::*;

/// The keychain service the agent server's tokens live under, so a person
/// can find them in their keychain app.
pub const KEYCHAIN_SERVICE: &str = "Beatlayer Agent Server";

/// The keychain account of the entry holding every token: one entry, so
/// the keychain is read once at start-up.
pub const KEYCHAIN_ACCOUNT: &str = "tokens";

/// The OS keychain, under [`KEYCHAIN_SERVICE`]: where the app keeps the
/// tokens.
#[must_use]
pub fn os_keychain() -> Arc<dyn Keychain> {
    Arc::new(OsKeychain::for_service(KEYCHAIN_SERVICE))
}

/// The tokens as the keychain entry holds them: the server's, and each
/// allowed agent's by name. It has no `Debug`, so it can't be printed.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct StoredTokens {
    server: String,
    clients: BTreeMap<String, String>,
}

impl StoredTokens {
    /// The tokens in `settings`, leaving out any not made yet.
    pub(crate) fn of(settings: &AgentSettings) -> Self {
        Self {
            server: settings.token.expose().to_owned(),
            clients: settings
                .clients
                .iter()
                .filter(|client| !client.token.is_empty())
                .map(|client| (client.name.clone(), client.token.expose().to_owned()))
                .collect(),
        }
    }

    /// Whether there are none.
    pub(crate) fn is_empty(&self) -> bool {
        self.server.is_empty() && self.clients.is_empty()
    }

    /// Reads the keychain entry's text.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|error| format!("its entry is damaged: {error}"))
    }

    /// The keychain entry's text.
    pub(crate) fn to_text(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Puts these tokens into `settings`: each one replaces what's there,
    /// and a token `settings` has that these don't is kept.
    pub(crate) fn apply(&self, settings: &mut AgentSettings) {
        if !self.server.is_empty() {
            settings.token = self.server.clone().into();
        }
        for client in &mut settings.clients {
            if let Some(token) = self.clients.get(&client.name) {
                client.token = token.clone().into();
            }
        }
    }
}

/// What the app says about the agent server, for the Settings window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentServerStatus {
    /// The address it's listening on, while it runs.
    pub listening: Option<String>,
    /// Why it isn't running when it should be, such as a port in use.
    pub error: Option<String>,
    /// How many agent sessions are open: one per client connection that
    /// initialised and hasn't ended.
    pub sessions: usize,
}

/// A new random token: 32 bytes from the operating system's secure random
/// source, as 64 hexadecimal digits.
///
/// # Errors
///
/// A message if the operating system's random source fails.
pub fn new_token() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| format!("couldn't make a random token: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_64_hex_digits_and_differ() {
        let a = new_token().unwrap();
        let b = new_token().unwrap();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}
