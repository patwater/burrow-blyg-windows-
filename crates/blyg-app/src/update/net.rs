//! The updater's only network access: HTTPS GETs to GitHub, behind a trait
//! so tests never touch the network.

use std::io::Read as _;
use std::time::Duration;

use super::UpdateError;

/// Hosts the updater may talk to: the Releases API, release asset URLs, and
/// the storage hosts GitHub redirects asset downloads to.
pub const ALLOWED_HOSTS: &[&str] = &[
    "api.github.com",
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

const MAX_REDIRECTS: usize = 5;

/// A GET that returns at most `limit` bytes.
pub trait Http: Send + Sync {
    fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>, UpdateError>;
}

/// Whether the updater may fetch `url`: HTTPS to an allowed host (or, in a
/// debug build pointed at a local test server, plain HTTP to localhost).
pub fn allowed(url: &str, allow_local: bool) -> Result<url::Url, UpdateError> {
    let u = url::Url::parse(url).map_err(|_| UpdateError::Network(format!("not a URL: {url}")))?;
    let host = u.host_str().unwrap_or_default();
    let local = allow_local
        && matches!(u.scheme(), "http" | "https")
        && matches!(host, "127.0.0.1" | "localhost");
    let github = u.scheme() == "https"
        && ALLOWED_HOSTS.contains(&host)
        && u.port().is_none()
        && u.username().is_empty()
        && u.password().is_none();
    if local || github {
        Ok(u)
    } else {
        Err(UpdateError::Network(format!(
            "refusing to download from {} (only HTTPS from GitHub)",
            u.origin().ascii_serialization()
        )))
    }
}

/// The real client: ureq (the same one blyg-core uses), following redirects
/// by hand so every hop is checked against `ALLOWED_HOSTS`.
pub struct UreqHttp {
    agent: ureq::Agent,
    allow_local: bool,
}

impl UreqHttp {
    pub fn new(allow_local: bool) -> Self {
        let agent = ureq::AgentBuilder::new()
            .redirects(0)
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(60))
            .user_agent(concat!(
                "Blygger-Desktop/",
                env!("CARGO_PKG_VERSION"),
                " (update check)"
            ))
            .build();
        Self { agent, allow_local }
    }
}

impl Http for UreqHttp {
    fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>, UpdateError> {
        let mut url = allowed(url, self.allow_local)?;
        for _ in 0..=MAX_REDIRECTS {
            let resp = match self
                .agent
                .request_url("GET", &url)
                .set("Accept", "application/vnd.github+json, */*")
                .call()
            {
                Ok(r) => r,
                Err(ureq::Error::Status(code, _)) => {
                    return Err(UpdateError::Network(format!("HTTP {code} from {url}")));
                }
                Err(e) => return Err(UpdateError::Network(e.kind().to_string())),
            };
            if (300..400).contains(&resp.status()) {
                let loc = resp
                    .header("Location")
                    .ok_or_else(|| UpdateError::Network("a redirect without Location".into()))?;
                let next = url
                    .join(loc)
                    .map_err(|_| UpdateError::Network("a bad redirect".into()))?;
                url = allowed(next.as_str(), self.allow_local)?;
                continue;
            }
            let mut body = Vec::new();
            resp.into_reader()
                .take(limit + 1)
                .read_to_end(&mut body)
                .map_err(|e| UpdateError::Network(e.to_string()))?;
            if body.len() as u64 > limit {
                return Err(UpdateError::Network(format!(
                    "{} is larger than expected",
                    url.path()
                )));
            }
            return Ok(body);
        }
        Err(UpdateError::Network("too many redirects".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_github_hosts_are_allowed() {
        for ok in [
            "https://api.github.com/repos/o/r/releases/latest",
            "https://github.com/o/r/releases/download/v1.0.0/SHA256SUMS",
            "https://objects.githubusercontent.com/github-production-release-asset/x",
            "https://release-assets.githubusercontent.com/github-production-release-asset/x",
        ] {
            assert!(allowed(ok, false).is_ok(), "{ok}");
        }
        for bad in [
            "http://github.com/o/r",
            "https://github.com.evil.example/o/r",
            "https://evil.example/github.com",
            "https://github.com:8443/o/r",
            "https://user@github.com/o/r",
            "file:///etc/passwd",
            "http://127.0.0.1:8000/latest",
            "not a url",
        ] {
            assert!(allowed(bad, false).is_err(), "{bad}");
        }
        // The debug-only local test server.
        assert!(allowed("http://127.0.0.1:8000/latest", true).is_ok());
        assert!(allowed("http://evil.example/latest", true).is_err());
    }
}
