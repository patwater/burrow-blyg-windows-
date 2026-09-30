//! Owner-token storage. The token never goes in a file: it lives in the
//! macOS Keychain under service `org.blygger.desktop`, account = the base
//! URL's host. Access goes through the `TokenStore` trait so tests use
//! `MemoryTokenStore` and never touch the real Keychain.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::backend::{CoreError, Result};

pub const KEYCHAIN_SERVICE: &str = "org.blygger.desktop";

/// Keychain account for a base URL: its host (`blyg.example.com`).
pub fn token_account(base_url: &str) -> String {
    url::Url::parse(base_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| base_url.trim().to_string())
}

pub trait TokenStore: Send + Sync {
    fn get(&self, base_url: &str) -> Result<Option<String>>;
    fn set(&self, base_url: &str, token: &str) -> Result<()>;
    fn delete(&self, base_url: &str) -> Result<()>;
}

/// In-memory token store (tests, `BLYGGER_FAKE`).
#[derive(Default)]
pub struct MemoryTokenStore(Mutex<HashMap<String, String>>);

impl TokenStore for MemoryTokenStore {
    fn get(&self, base_url: &str) -> Result<Option<String>> {
        Ok(self
            .0
            .lock()
            .map_err(|_| CoreError::Other("poisoned".into()))?
            .get(&token_account(base_url))
            .cloned())
    }
    fn set(&self, base_url: &str, token: &str) -> Result<()> {
        self.0
            .lock()
            .map_err(|_| CoreError::Other("poisoned".into()))?
            .insert(token_account(base_url), token.to_string());
        Ok(())
    }
    fn delete(&self, base_url: &str) -> Result<()> {
        self.0
            .lock()
            .map_err(|_| CoreError::Other("poisoned".into()))?
            .remove(&token_account(base_url));
        Ok(())
    }
}

/// The real macOS Keychain (feature `keychain`, on by default).
#[cfg(feature = "keychain")]
#[derive(Debug, Default, Clone, Copy)]
pub struct KeychainTokenStore;

#[cfg(feature = "keychain")]
impl KeychainTokenStore {
    fn entry(base_url: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(KEYCHAIN_SERVICE, &token_account(base_url))
            .map_err(|e| CoreError::Other(format!("keychain: {e}")))
    }
}

#[cfg(feature = "keychain")]
impl TokenStore for KeychainTokenStore {
    fn get(&self, base_url: &str) -> Result<Option<String>> {
        match Self::entry(base_url)?.get_password() {
            Ok(t) => Ok(Some(t)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(CoreError::Other(format!("keychain: {e}"))),
        }
    }
    fn set(&self, base_url: &str, token: &str) -> Result<()> {
        Self::entry(base_url)?
            .set_password(token)
            .map_err(|e| CoreError::Other(format!("keychain: {e}")))
    }
    fn delete(&self, base_url: &str) -> Result<()> {
        match Self::entry(base_url)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(CoreError::Other(format!("keychain: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_account_is_host() {
        assert_eq!(
            token_account("https://blyg.example.com/"),
            "blyg.example.com"
        );
        assert_eq!(token_account("http://127.0.0.1:8787"), "127.0.0.1");
    }

    #[test]
    fn memory_token_store() {
        let s = MemoryTokenStore::default();
        assert_eq!(s.get("https://a.example").unwrap(), None);
        s.set("https://a.example/", "t0k").unwrap();
        assert_eq!(s.get("https://a.example").unwrap().as_deref(), Some("t0k"));
        s.delete("https://a.example").unwrap();
        assert_eq!(s.get("https://a.example").unwrap(), None);
    }
}
