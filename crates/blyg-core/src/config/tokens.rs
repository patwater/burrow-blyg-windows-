//! Owner-credential storage. No credential ever goes in a file: they live
//! in the macOS Keychain under service `org.blygger.desktop`, account =
//!
//! - `<host> oauth`: a browser sign-in's grant (JSON: tokens, expiry,
//!   where to renew it);
//! - `<host>`: an API token;
//! - `<host> password`: the studio password;
//! - `<host> oauth-client`: the client id Burrow registered with that blyg
//!   (not a secret; kept across sign-outs so it's reused).
//!
//! A blyg has one credential at a time; when more than one is found, the
//! browser sign-in wins, then the token, then the password. Access goes
//! through the `TokenStore` trait so tests use `MemoryTokenStore` and never
//! touch the real Keychain.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::api::auth::Credential;
use crate::api::oauth::{OAuthGrant, OAuthSession, grant_key};
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

/// The `TokenStore` key a blyg's studio password is kept under: its own
/// Keychain account, `<host> password`, next to the token's `<host>`.
pub fn password_key(base_url: &str) -> String {
    format!("{} password", token_account(base_url))
}

/// The browser sign-in kept for a blyg, if any (also when it has ended).
pub fn load_grant(store: &dyn TokenStore, base_url: &str) -> Result<Option<OAuthGrant>> {
    Ok(store
        .get(&grant_key(base_url))?
        .and_then(|s| serde_json::from_str(&s).ok()))
}

/// The credential stored for a blyg: its browser sign-in, else its API
/// token, else its studio password. A browser sign-in renews itself and
/// saves each renewal back to `store`.
pub fn load_credential(store: &Arc<dyn TokenStore>, base_url: &str) -> Result<Option<Credential>> {
    if let Some(g) = load_grant(store.as_ref(), base_url)? {
        return Ok(Some(Credential::OAuth(OAuthSession::kept(
            g,
            store.clone(),
            base_url,
        ))));
    }
    if let Some(t) = store.get(base_url)? {
        return Ok(Some(Credential::Token(t)));
    }
    Ok(store
        .get(&password_key(base_url))?
        .map(Credential::Password))
}

/// Keep `cred` for a blyg, and drop the other kinds, so there's one.
pub fn save_credential(store: &dyn TokenStore, base_url: &str, cred: &Credential) -> Result<()> {
    let (keep, value) = match cred {
        Credential::Token(t) => (base_url.to_string(), t.clone()),
        Credential::Password(p) => (password_key(base_url), p.clone()),
        Credential::OAuth(s) => (
            grant_key(base_url),
            serde_json::to_string(&s.grant()).map_err(|e| CoreError::Other(e.to_string()))?,
        ),
    };
    store.set(&keep, &value)?;
    for key in credential_keys(base_url) {
        if key != keep {
            store.delete(&key)?;
        }
    }
    Ok(())
}

/// Forget every credential kept for a blyg (the grant, the token and the
/// password). The registered client id stays, to be reused.
pub fn delete_credential(store: &dyn TokenStore, base_url: &str) -> Result<()> {
    for key in credential_keys(base_url) {
        store.delete(&key)?;
    }
    Ok(())
}

fn credential_keys(base_url: &str) -> [String; 3] {
    [
        grant_key(base_url),
        base_url.to_string(),
        password_key(base_url),
    ]
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
    fn credentials_are_kept_apart_and_one_at_a_time() {
        let mem = Arc::new(MemoryTokenStore::default());
        let s: Arc<dyn TokenStore> = mem.clone();
        let base = "https://blyg.example.com/";
        assert_eq!(password_key(base), "blyg.example.com password");
        assert_eq!(grant_key(base), "blyg.example.com oauth");
        assert_eq!(load_credential(&s, base).unwrap(), None);
        save_credential(&*s, base, &Credential::Token("t".into())).unwrap();
        assert_eq!(
            load_credential(&s, base).unwrap(),
            Some(Credential::Token("t".into()))
        );
        save_credential(&*s, base, &Credential::Password("p".into())).unwrap();
        assert_eq!(
            load_credential(&s, base).unwrap(),
            Some(Credential::Password("p".into()))
        );
        assert_eq!(s.get(base).unwrap(), None, "the token is dropped");
        let grant = OAuthGrant {
            access: "a".into(),
            refresh: Some("r".into()),
            expires: 1,
            deadline: 0,
            scope: "owner:read".into(),
            client_id: "cid".into(),
            token_endpoint: "https://blyg.example.com/t".into(),
            revocation_endpoint: None,
            resource: "https://blyg.example.com/api".into(),
            invalid: false,
        };
        save_credential(
            &*s,
            base,
            &Credential::OAuth(OAuthSession::new(grant.clone())),
        )
        .unwrap();
        assert_eq!(
            s.get(&password_key(base)).unwrap(),
            None,
            "the password is dropped"
        );
        match load_credential(&s, base).unwrap() {
            Some(Credential::OAuth(o)) => assert_eq!(o.grant(), grant),
            other => panic!("{other:?}"),
        }
        // More than one kept (an older version): browser, token, password.
        s.set(base, "t").unwrap();
        s.set(&password_key(base), "p").unwrap();
        assert!(matches!(
            load_credential(&s, base).unwrap(),
            Some(Credential::OAuth(_))
        ));
        s.delete(&grant_key(base)).unwrap();
        assert_eq!(
            load_credential(&s, base).unwrap(),
            Some(Credential::Token("t".into()))
        );
        s.set(&grant_key(base), &serde_json::to_string(&grant).unwrap())
            .unwrap();
        delete_credential(&*s, base).unwrap();
        assert_eq!(load_credential(&s, base).unwrap(), None);
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
