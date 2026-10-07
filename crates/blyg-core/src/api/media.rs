//! Reading the blyg's own media with the owner credential (studio 0.28):
//! `GET {base}/media/{file}` for an upload no public version uses yet is a
//! 404 unless it carries `Authorization: Bearer` (owner:read) or the owner
//! cookie. The credential only ever goes to the blyg's own origin, never
//! follows a redirect, and is never logged.

use std::io::Read;
use std::time::Duration;

use super::{Api, auth};
use crate::backend::{CoreError, Result};

impl Api {
    /// The owner credential as one request header, `(name, value)`:
    /// `authorization: Bearer …` for a token or a browser sign-in (renewed
    /// first when it's about to expire), or the studio session cookie
    /// (signing in first) for a password. Never log the value.
    pub fn auth_header(&self) -> Option<(&'static str, String)> {
        match &self.cred {
            auth::Credential::Token(t) => Some(("authorization", format!("Bearer {t}"))),
            auth::Credential::Password(p) => self
                .session(p)
                .ok()
                .map(|s| ("cookie", format!("{}={s}", auth::SESSION_COOKIE))),
            auth::Credential::OAuth(s) => s
                .access_token()
                .ok()
                .map(|t| ("authorization", format!("Bearer {t}"))),
        }
    }

    /// Whether `url` is media on this blyg's own origin (`…/media/<file>`),
    /// the only place the credential may go.
    pub fn is_own_media(&self, url: &str) -> bool {
        let (Ok(u), Ok(api)) = (url::Url::parse(url), url::Url::parse(&self.api_root)) else {
            return false;
        };
        matches!(u.scheme(), "http" | "https")
            && u.origin() == api.origin()
            && u.path().contains("/media/")
            && u.username().is_empty()
            && u.password().is_none()
    }

    /// `GET` one of the blyg's own media files with the owner credential:
    /// (bytes, content type). `url` must pass [`Api::is_own_media`]; at
    /// most `max` bytes. 404/410 → `NotFound`.
    pub fn fetch_own_media(&self, url: &str, max: u64) -> Result<(Vec<u8>, Option<String>)> {
        if !self.is_own_media(url) {
            return Err(CoreError::Other("not this blyg's media".into()));
        }
        // No redirects: a redirect could carry the credential elsewhere.
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(30))
            .redirects(0)
            .build();
        let mut req = agent.get(url);
        if let Some((name, value)) = self.auth_header() {
            req = req.set(name, &value);
        }
        match req.call() {
            Ok(r) if r.status() == 200 => {
                let mime = r
                    .header("content-type")
                    .map(|m| m.split(';').next().unwrap_or(m).trim().to_ascii_lowercase());
                let mut bytes = Vec::new();
                r.into_reader()
                    .take(max + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| CoreError::Offline)?;
                if bytes.len() as u64 > max {
                    return Err(CoreError::Other(format!("larger than {max} bytes")));
                }
                Ok((bytes, mime))
            }
            Ok(r) => Err(CoreError::Rejected {
                status: r.status(),
                message: format!("media returned {}", r.status()),
                details: vec![],
            }),
            Err(ureq::Error::Transport(_)) => Err(CoreError::Offline),
            Err(ureq::Error::Status(401, _)) => Err(CoreError::Unauthorized),
            Err(ureq::Error::Status(404 | 410, _)) => Err(CoreError::NotFound),
            Err(ureq::Error::Status(code, _)) => Err(CoreError::Rejected {
                status: code,
                message: format!("media returned {code}"),
                details: vec![],
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_goes_only_to_the_blygs_own_media() {
        let api = Api::new(
            "https://blyg.example.com/blyg",
            auth::Credential::Token("t".into()),
        );
        assert!(api.is_own_media("https://blyg.example.com/blyg/media/a.png"));
        assert!(api.is_own_media("https://blyg.example.com/media/a.png"));
        assert!(!api.is_own_media("http://blyg.example.com/blyg/media/a.png"));
        assert!(!api.is_own_media("https://blyg.example.com.evil.example/media/a.png"));
        assert!(!api.is_own_media("https://other.example.org/blyg/media/a.png"));
        assert!(!api.is_own_media("https://u:p@blyg.example.com/media/a.png"));
        assert!(!api.is_own_media("https://blyg.example.com/blyg/f/abc"));
        assert!(!api.is_own_media("media/a.png"));
    }
}
