//! The agent server's settings at work (WP-1.15): its token, and what the
//! app reports about the running server for the Settings window.
//!
//! The commands that change the settings are in [`bl_shell_spec::agent`];
//! [`Shell::run`](crate::Shell::run) runs them. The server itself is the
//! agent API's (`bl-agent`), started and stopped by the app as the settings
//! say.

pub use bl_shell_spec::agent::*;

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
