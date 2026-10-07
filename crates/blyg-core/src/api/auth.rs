//! How the app proves it's the owner. Three ways, in order of preference:
//!
//! - **Browser sign-in** (studio 0.28+): an OAuth grant with a one-hour
//!   access token and a rotating refresh token, sent as
//!   `Authorization: Bearer` and renewed as it goes (see `oauth`).
//! - **API token**: a manual token the owner makes in Studio → More →
//!   Client access (studio 0.28+; 30 days, not renewable), or an older
//!   Worker fork's `BLYG_OWNER_TOKEN`. Sent as `Authorization: Bearer`.
//! - **Password**, which works on any blygger-studio: the studio's own
//!   sign-in, `POST {blyg-url}/studio/login` with the form field `password`,
//!   answers with a `blyg_session` cookie (valid 30 days, not renewed). The
//!   app signs in once per launch, keeps the cookie in memory only, and signs
//!   in again when the API answers 401. Studio 0.28+ budgets password
//!   sign-ins and means its cookie for browsers, so this is the fallback.
//!
//! No secret is ever logged, printed or put in an error: `Debug` redacts
//! them.

use std::time::Duration;

use super::oauth::OAuthSession;
use crate::backend::{CoreError, Result};

/// The studio's session cookie (upstream `src/auth.ts` `COOKIE_NAME`).
pub const SESSION_COOKIE: &str = "blyg_session";

#[derive(Clone, PartialEq, Eq)]
pub enum Credential {
    /// A manual API token (studio 0.28+), or an older Worker fork's
    /// `BLYG_OWNER_TOKEN`.
    Token(String),
    /// The studio password (the Worker's `OWNER_PASSWORD`).
    Password(String),
    /// A browser sign-in's grant (studio 0.28+).
    OAuth(OAuthSession),
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Credential::Token(_) => "Token(<redacted>)",
            Credential::Password(_) => "Password(<redacted>)",
            Credential::OAuth(_) => "OAuth(<redacted>)",
        })
    }
}

impl From<&str> for Credential {
    fn from(token: &str) -> Self {
        Credential::Token(token.to_string())
    }
}

impl From<String> for Credential {
    fn from(token: String) -> Self {
        Credential::Token(token)
    }
}

impl From<&String> for Credential {
    fn from(token: &String) -> Self {
        Credential::Token(token.clone())
    }
}

/// Sign in to the studio at `base` (the blyg URL, with its mount) and return
/// the session cookie's value. A wrong password is `Unauthorized`; no studio
/// there is `NotFound`; nothing answering is `Offline`.
pub fn login(base: &str, password: &str) -> Result<String> {
    // Not following the redirect: the cookie is on the 302 itself.
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .redirects(0)
        .build();
    let url = format!("{}/studio/login", base.trim_end_matches('/'));
    match agent.post(&url).send_form(&[("password", password)]) {
        Ok(r) => r
            .all("set-cookie")
            .into_iter()
            .find_map(session_value)
            .ok_or_else(|| {
                // The studio answered but set no session: not a blygger-studio
                // sign-in, or it refused without saying so.
                CoreError::Other("the blyg's sign-in page didn't start a session".into())
            }),
        Err(ureq::Error::Status(401 | 403, _)) => Err(CoreError::Unauthorized),
        Err(ureq::Error::Status(404, _)) => Err(CoreError::NotFound),
        Err(ureq::Error::Status(code, _)) => Err(CoreError::Rejected {
            status: code,
            message: format!("signing in failed ({code})"),
            details: vec![],
        }),
        Err(ureq::Error::Transport(_)) => Err(CoreError::Offline),
    }
}

/// `blyg_session=<value>; Path=/; …` → `<value>`; `None` for other cookies,
/// or a cleared one.
fn session_value(header: &str) -> Option<String> {
    let first = header.split(';').next()?.trim();
    let (name, value) = first.split_once('=')?;
    (name.trim() == SESSION_COOKIE && !value.trim().is_empty()).then(|| value.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_session_cookie_only() {
        assert_eq!(
            session_value(
                "blyg_session=123.abc; Path=/; Max-Age=2592000; HttpOnly; Secure; SameSite=Lax"
            )
            .as_deref(),
            Some("123.abc")
        );
        assert_eq!(session_value("other=1; Path=/"), None);
        assert_eq!(session_value("blyg_session=; Path=/; Max-Age=0"), None);
    }

    #[test]
    fn debug_never_shows_a_secret() {
        let grant = crate::api::oauth::OAuthGrant {
            access: "acc-secret".into(),
            refresh: Some("ref-secret".into()),
            expires: 0,
            deadline: 0,
            scope: String::new(),
            client_id: "cid".into(),
            token_endpoint: "https://blyg.example.com/t".into(),
            revocation_endpoint: None,
            resource: "https://blyg.example.com/api".into(),
            invalid: false,
        };
        let s = format!(
            "{:?} {:?} {:?}",
            Credential::Token("tok-secret".into()),
            Credential::Password("pw-secret".into()),
            Credential::OAuth(OAuthSession::new(grant)),
        );
        assert!(!s.contains("secret"), "{s}");
    }
}
