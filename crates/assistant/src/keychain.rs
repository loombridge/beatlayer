//! API keys, in the OS keychain and nowhere else.
//!
//! A key is written and read through [`Keychain`]: the OS keychain
//! (macOS Keychain, Windows Credential Manager, the Secret Service on
//! Linux) through [`OsKeychain`], or a stand-in for tests and headless runs
//! ([`MemoryKeychain`]). No settings or project file ever holds a key.
//!
//! Each provider names the account its key lives under
//! ([`OpenAi::ACCOUNT`](crate::provider::OpenAi::ACCOUNT) and the others),
//! all of them under one service, [`SERVICE`], so a person can find them in
//! their keychain app. The agent server's tokens use the same keychain under
//! a service of their own ([`OsKeychain::for_service`]).

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Mutex;

/// The service every assistant API key lives under.
pub const SERVICE: &str = "Beatlayer Assistant";

/// Where a provider's API key is stored and read.
pub trait Keychain: Send + Sync {
    /// The key stored for `account`, or `None` if there is none.
    ///
    /// # Errors
    ///
    /// If the keychain can't be read (see [`KeychainError`]).
    fn read(&self, account: &str) -> Result<Option<String>, KeychainError>;

    /// Stores `key` for `account`, replacing what was there.
    ///
    /// # Errors
    ///
    /// If the keychain can't be written (see [`KeychainError`]).
    fn write(&self, account: &str, key: &str) -> Result<(), KeychainError>;

    /// Removes what is stored for `account`. Removing nothing is fine.
    ///
    /// # Errors
    ///
    /// If the keychain can't be written (see [`KeychainError`]).
    fn delete(&self, account: &str) -> Result<(), KeychainError>;
}

/// Why the keychain didn't answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeychainError {
    /// There's no keychain to talk to: on Linux a desktop without a running
    /// Secret Service (no D-Bus session), or a locked one.
    Unavailable(String),
    /// The keychain itself failed.
    Failed(String),
}

impl fmt::Display for KeychainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(why) => write!(f, "no OS keychain is available: {why}"),
            Self::Failed(why) => write!(f, "the OS keychain failed: {why}"),
        }
    }
}

impl std::error::Error for KeychainError {}

/// The OS keychain: macOS Keychain, Windows Credential Manager, the Secret
/// Service on Linux.
#[derive(Clone, Copy, Debug)]
pub struct OsKeychain {
    /// The service its entries live under.
    service: &'static str,
}

impl Default for OsKeychain {
    fn default() -> Self {
        Self::new()
    }
}

impl OsKeychain {
    /// The OS keychain, for the assistant's API keys under [`SERVICE`].
    #[must_use]
    pub fn new() -> Self {
        Self::for_service(SERVICE)
    }

    /// The OS keychain, for entries under another service, such as the
    /// agent server's tokens.
    #[must_use]
    pub fn for_service(service: &'static str) -> Self {
        Self { service }
    }

    /// The entry for `account`, with the platform's store behind it.
    fn entry(&self, account: &str) -> Result<keyring::Entry, KeychainError> {
        keyring::Entry::new(self.service, account).map_err(map_error)
    }
}

/// Maps the store's error onto ours, keeping "there's none" out of it: that
/// isn't an error.
fn map_error(error: keyring::Error) -> KeychainError {
    match error {
        keyring::Error::NoDefaultStore => KeychainError::Unavailable(
            "the platform's credential store couldn't be started; see the keyring crate's \
             `Entry::store_status`"
                .to_owned(),
        ),
        keyring::Error::NoStorageAccess(why) => KeychainError::Unavailable(why.to_string()),
        other => KeychainError::Failed(other.to_string()),
    }
}

impl Keychain for OsKeychain {
    fn read(&self, account: &str) -> Result<Option<String>, KeychainError> {
        match self.entry(account)?.get_password() {
            Ok(key) => Ok(Some(key)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(map_error(error)),
        }
    }

    fn write(&self, account: &str, key: &str) -> Result<(), KeychainError> {
        self.entry(account)?.set_password(key).map_err(map_error)
    }

    fn delete(&self, account: &str) -> Result<(), KeychainError> {
        match self.entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(map_error(error)),
        }
    }
}

/// Keys in memory only, for tests and headless runs: nothing reaches the OS
/// keychain, and everything is gone when it drops.
#[derive(Debug, Default)]
pub struct MemoryKeychain {
    keys: Mutex<BTreeMap<String, String>>,
}

impl MemoryKeychain {
    /// An empty in-memory keychain.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl Keychain for MemoryKeychain {
    fn read(&self, account: &str) -> Result<Option<String>, KeychainError> {
        let keys = self
            .keys
            .lock()
            .expect("the keychain's lock is never poisoned");
        Ok(keys.get(account).cloned())
    }

    fn write(&self, account: &str, key: &str) -> Result<(), KeychainError> {
        let mut keys = self
            .keys
            .lock()
            .expect("the keychain's lock is never poisoned");
        keys.insert(account.to_owned(), key.to_owned());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), KeychainError> {
        let mut keys = self
            .keys
            .lock()
            .expect("the keychain's lock is never poisoned");
        keys.remove(account);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_written_is_a_key_read_back() {
        let keychain = MemoryKeychain::new();
        assert_eq!(keychain.read("openai").unwrap(), None);
        keychain.write("openai", "sk-test").unwrap();
        assert_eq!(keychain.read("openai").unwrap(), Some("sk-test".to_owned()));
        assert_eq!(keychain.read("anthropic").unwrap(), None);
        keychain.write("openai", "sk-other").unwrap();
        assert_eq!(
            keychain.read("openai").unwrap(),
            Some("sk-other".to_owned())
        );
        keychain.delete("openai").unwrap();
        assert_eq!(keychain.read("openai").unwrap(), None);
        // Deleting nothing is fine.
        keychain.delete("openai").unwrap();
    }

    #[test]
    fn an_unavailable_keychain_says_so() {
        let error = KeychainError::Unavailable("no D-Bus session".to_owned());
        assert!(error.to_string().contains("no D-Bus session"), "{error}");
    }
}
