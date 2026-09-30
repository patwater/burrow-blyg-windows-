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

#[cfg(all(feature = "keychain", not(windows)))]
impl KeychainTokenStore {
    fn entry(base_url: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(KEYCHAIN_SERVICE, &token_account(base_url))
            .map_err(|e| CoreError::Other(format!("keychain: {e}")))
    }
}

#[cfg(all(feature = "keychain", not(windows)))]
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

/// Windows Credential Manager (`keyring`'s `windows-native`), which holds
/// at most 2,560 bytes (1,280 UTF-16 units) per secret: less than a ChatGPT
/// sign-in. Longer secrets are split, see `chunked`.
#[cfg(all(feature = "keychain", windows))]
impl TokenStore for KeychainTokenStore {
    fn get(&self, base_url: &str) -> Result<Option<String>> {
        chunked::get(&CredentialManager, &token_account(base_url))
    }
    fn set(&self, base_url: &str, token: &str) -> Result<()> {
        chunked::set(&CredentialManager, &token_account(base_url), token)
    }
    fn delete(&self, base_url: &str) -> Result<()> {
        chunked::delete(&CredentialManager, &token_account(base_url))
    }
}

/// One Credential Manager entry per account name, no splitting.
#[cfg(all(feature = "keychain", windows))]
struct CredentialManager;

#[cfg(all(feature = "keychain", windows))]
impl chunked::Raw for CredentialManager {
    fn get(&self, account: &str) -> Result<Option<String>> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, account)
            .map_err(|e| CoreError::Other(format!("keychain: {e}")))?;
        match entry.get_password() {
            Ok(t) => Ok(Some(t)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(CoreError::Other(format!("keychain: {e}"))),
        }
    }
    fn set(&self, account: &str, value: &str) -> Result<()> {
        keyring::Entry::new(KEYCHAIN_SERVICE, account)
            .and_then(|e| e.set_password(value))
            .map_err(|e| CoreError::Other(format!("keychain: {e}")))
    }
    fn delete(&self, account: &str) -> Result<()> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, account)
            .map_err(|e| CoreError::Other(format!("keychain: {e}")))?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(CoreError::Other(format!("keychain: {e}"))),
        }
    }
}

/// Secrets too long for one Credential Manager entry: the parts go in
/// `account#1`, `account#2`…, and `account` itself holds a marker with
/// the count. Short secrets are stored as they are.
#[cfg_attr(not(all(feature = "keychain", windows)), allow(dead_code))]
mod chunked {
    use crate::backend::{CoreError, Result};

    /// 600 chars is at most 1,200 UTF-16 units, under the 1,280 limit.
    pub const PART_CHARS: usize = 600;
    const MARKER: &str = "\u{1}blygger-parts:";

    pub trait Raw {
        fn get(&self, account: &str) -> Result<Option<String>>;
        fn set(&self, account: &str, value: &str) -> Result<()>;
        fn delete(&self, account: &str) -> Result<()>;
    }

    fn part(account: &str, i: usize) -> String {
        format!("{account}#{i}")
    }

    fn parts_in(head: &str) -> Option<usize> {
        head.strip_prefix(MARKER)?.parse().ok()
    }

    pub fn get(raw: &dyn Raw, account: &str) -> Result<Option<String>> {
        let Some(head) = raw.get(account)? else {
            return Ok(None);
        };
        let Some(n) = parts_in(&head) else {
            return Ok(Some(head));
        };
        let mut secret = String::new();
        for i in 1..=n {
            match raw.get(&part(account, i))? {
                Some(p) => secret.push_str(&p),
                None => {
                    return Err(CoreError::Other(format!(
                        "keychain: part {i} of {n} for {account} is missing"
                    )));
                }
            }
        }
        Ok(Some(secret))
    }

    pub fn set(raw: &dyn Raw, account: &str, secret: &str) -> Result<()> {
        delete(raw, account)?;
        let chars: Vec<char> = secret.chars().collect();
        if chars.len() <= PART_CHARS {
            return raw.set(account, secret);
        }
        let parts: Vec<String> = chars
            .chunks(PART_CHARS)
            .map(|c| c.iter().collect())
            .collect();
        for (i, p) in parts.iter().enumerate() {
            raw.set(&part(account, i + 1), p)?;
        }
        raw.set(account, &format!("{MARKER}{}", parts.len()))
    }

    pub fn delete(raw: &dyn Raw, account: &str) -> Result<()> {
        if let Some(n) = raw.get(account)?.as_deref().and_then(parts_in) {
            for i in 1..=n {
                raw.delete(&part(account, i))?;
            }
        }
        raw.delete(account)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A raw store that refuses what Credential Manager would.
    #[derive(Default)]
    struct Limited(Mutex<HashMap<String, String>>);

    impl chunked::Raw for Limited {
        fn get(&self, account: &str) -> Result<Option<String>> {
            Ok(self.0.lock().unwrap().get(account).cloned())
        }
        fn set(&self, account: &str, value: &str) -> Result<()> {
            if value.encode_utf16().count() > 1280 {
                return Err(CoreError::Other("too long".into()));
            }
            self.0.lock().unwrap().insert(account.into(), value.into());
            Ok(())
        }
        fn delete(&self, account: &str) -> Result<()> {
            self.0.lock().unwrap().remove(account);
            Ok(())
        }
    }

    #[test]
    fn long_secrets_are_split_and_joined() {
        let raw = Limited::default();
        let long: String = "a😀bc".repeat(1000);
        chunked::set(&raw, "chatgpt", &long).unwrap();
        assert_eq!(chunked::get(&raw, "chatgpt").unwrap(), Some(long));
        assert!(raw.0.lock().unwrap().len() > 2, "stored in parts");

        // Shorter again: the old parts go.
        chunked::set(&raw, "chatgpt", "short").unwrap();
        assert_eq!(
            chunked::get(&raw, "chatgpt").unwrap().as_deref(),
            Some("short")
        );
        assert_eq!(raw.0.lock().unwrap().len(), 1);

        chunked::delete(&raw, "chatgpt").unwrap();
        assert_eq!(chunked::get(&raw, "chatgpt").unwrap(), None);
        assert!(raw.0.lock().unwrap().is_empty());
    }

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
