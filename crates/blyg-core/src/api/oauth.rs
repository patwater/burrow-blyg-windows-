//! Sign in with the browser: OAuth 2 for desktop apps (RFC 8252) against a
//! blygger-studio 0.28 or later.
//!
//! 1. **Discover** `{blyg-url}/studio/auth/.well-known/openid-configuration`
//!    and the API's resource metadata (`…/studio/auth/resources/api`). A
//!    studio without them (older than 0.28) has no browser sign-in.
//! 2. **Register** a public client (`token_endpoint_auth_method: none`,
//!    `application_type: native`) named "Burrow" once per blyg, with the
//!    loopback redirect URIs [`LOOPBACK_PORTS`]. Its id is kept (not secret)
//!    under the `TokenStore` account `<host> oauth-client` and reused; a kept
//!    id the server no longer knows (unapproved clients are reclaimed after a
//!    day) is replaced.
//! 3. **Authorize** with PKCE (S256) and `state` in the system browser; the
//!    owner signs in to the studio there and allows Burrow. The code comes
//!    back to a listener on 127.0.0.1 ([`Loopback`]).
//! 4. **Exchange** the code for a one-hour access token and a rotating
//!    refresh token, bound to the API resource, with all four owner scopes.
//!
//! The grant ([`OAuthGrant`]) is kept as JSON under `<host> oauth`. An
//! [`OAuthSession`] renews it shortly before it expires and after a 401,
//! saving each rotation at once: a refresh token is good for one use, and
//! the studio revokes the whole grant when an old one comes back. A refused
//! refresh, or the grant's 30-day deadline (the `grantExpires` claim), marks
//! the grant `invalid`: every call then fails with `Unauthorized`, without
//! touching the network, until the owner signs in again.
//!
//! Tokens are never logged or printed: `Debug` redacts them.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::backend::{CoreError, Result};
use crate::config::{TokenStore, token_account};

/// The owner scopes (upstream `src/permissions.ts` `OWNER_SCOPES`).
pub const OWNER_SCOPES: [&str; 4] = ["owner:read", "owner:draft", "owner:publish", "owner:manage"];
/// What Burrow asks for: every owner scope, and a refresh token.
pub const SCOPE: &str = "owner:read owner:draft owner:publish owner:manage offline_access";
/// The name the studio's consent page shows (self-asserted).
pub const CLIENT_NAME: &str = "Burrow";
/// The loopback callback's path.
pub const CALLBACK_PATH: &str = "/oauth/callback";
/// The callback ports registered with every blyg, tried in order. When all
/// are taken, any free port works too: the studio matches loopback redirect
/// URIs without their port (RFC 8252 §7.3).
pub const LOOPBACK_PORTS: [u16; 3] = [47811, 47812, 47813];
/// How long to wait for the owner to allow Burrow in the browser.
pub const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Renew this long before the access token expires.
const RENEW_MARGIN: u64 = 120;

/// The `TokenStore` key for a blyg's grant: `<host> oauth`.
pub fn grant_key(base_url: &str) -> String {
    format!("{} oauth", token_account(base_url))
}

/// The `TokenStore` key for a blyg's registered client: `<host> oauth-client`.
pub fn client_key(base_url: &str) -> String {
    format!("{} oauth-client", token_account(base_url))
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .redirects(0)
        .build()
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::getrandom(&mut b).expect("OS randomness");
    b
}

/// A PKCE verifier (43 URL-safe characters) and its S256 challenge.
pub fn pkce() -> (String, String) {
    let verifier = URL_SAFE_NO_PAD.encode(random_bytes::<32>());
    let challenge = s256(&verifier);
    (verifier, challenge)
}

/// `BASE64URL(SHA256(verifier))`, unpadded (RFC 7636 §4.2).
pub fn s256(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// An unguessable `state`.
pub fn random_state() -> String {
    URL_SAFE_NO_PAD.encode(random_bytes::<24>())
}

/// A loopback redirect URI.
pub fn redirect_uri(port: u16) -> String {
    format!("http://127.0.0.1:{port}{CALLBACK_PATH}")
}

/// The redirect URIs every Burrow client registers.
pub fn registered_redirect_uris() -> Vec<String> {
    LOOPBACK_PORTS.iter().map(|&p| redirect_uri(p)).collect()
}

/// A numeric claim from a JWT's payload (unverified: the server checks
/// the signature; this only reads what it told us).
pub fn jwt_claim_u64(token: &str, claim: &str) -> Option<u64> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v.get(claim)?.as_u64()
}

/// The scopes a 403's `WWW-Authenticate` asks for, when it says
/// `error="insufficient_scope"` (upstream `bearerChallenge`).
pub fn insufficient_scope(www_authenticate: &str) -> Option<Vec<String>> {
    if !www_authenticate.contains("insufficient_scope") {
        return None;
    }
    let scope = auth_param(www_authenticate, "scope").unwrap_or_default();
    Some(scope.split_whitespace().map(str::to_string).collect())
}

/// One `name="value"` parameter of a challenge.
fn auth_param(header: &str, name: &str) -> Option<String> {
    let mut from = 0;
    while let Some(i) = header[from..].find(name).map(|i| from + i) {
        let before = header[..i].chars().next_back();
        let after = &header[i + name.len()..];
        if matches!(before, None | Some(' ' | ','))
            && let Some(v) = after.strip_prefix("=\"")
        {
            return v.split_once('"').map(|(v, _)| v.to_string());
        }
        from = i + name.len();
    }
    None
}

/// What to say when the blyg answered 403 "insufficient scope". `granted`
/// is the grant's scopes when known (browser sign-in); a manual token's
/// aren't, so it names everything the call needed.
pub fn missing_scope_message(required: &[String], granted: Option<&str>) -> String {
    let missing: Vec<&str> = required
        .iter()
        .map(String::as_str)
        .filter(|s| granted.is_none_or(|g| !g.split_whitespace().any(|x| x == *s)))
        .collect();
    let named = if missing.is_empty() {
        "a permission it doesn't have".to_string()
    } else {
        format!(
            "the {} permission{}",
            missing.join(" and "),
            if missing.len() == 1 { "" } else { "s" }
        )
    };
    match granted {
        Some(_) => format!(
            "The blyg refused: Burrow's sign-in lacks {named}. Sign in with the browser again and allow all four permissions."
        ),
        None => format!(
            "The blyg refused: this token lacks {named}. Make a new one in Studio → More → Client access, for the REST API, with all four permissions (read, draft, publish, manage)."
        ),
    }
}

// ----------------------------------------------------------- discovery

/// Where a blyg's authorization server is, from its discovery documents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovery {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: String,
    pub revocation_endpoint: Option<String>,
    /// The REST API's resource identifier (`{origin}/api`).
    pub resource: String,
}

/// `{blyg-url}/studio/auth/.well-known/openid-configuration`.
pub fn discovery_url(base_url: &str) -> String {
    format!(
        "{}/studio/auth/.well-known/openid-configuration",
        base_url.trim_end_matches('/')
    )
}

fn get_json(url: &str) -> Result<Option<Value>> {
    match agent().get(url).set("accept", "application/json").call() {
        Ok(r) => Ok(r.into_json::<Value>().ok()),
        Err(ureq::Error::Status(_, _)) => Ok(None),
        Err(ureq::Error::Transport(_)) => Err(CoreError::Offline),
    }
}

/// The blyg's OAuth endpoints, or `None` when it has no browser sign-in
/// (a studio older than 0.28, or not a studio).
pub fn discover(base_url: &str) -> Result<Option<Discovery>> {
    let Some(doc) = get_json(&discovery_url(base_url))? else {
        return Ok(None);
    };
    let s = |k: &str| doc.get(k).and_then(Value::as_str).map(str::to_string);
    let s256 = doc
        .get("code_challenge_methods_supported")
        .and_then(Value::as_array)
        .is_some_and(|a| a.iter().any(|m| m == "S256"));
    let (Some(issuer), Some(authorization_endpoint), Some(token_endpoint), Some(registration)) = (
        s("issuer"),
        s("authorization_endpoint"),
        s("token_endpoint"),
        s("registration_endpoint"),
    ) else {
        return Ok(None);
    };
    if !s256 {
        return Ok(None);
    }
    // The API resource the server advertises; else upstream's `{origin}/api`.
    let metadata = format!("{}/resources/api", issuer.trim_end_matches('/'));
    let resource = get_json(&metadata)?
        .and_then(|v| {
            v.get("resource")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| format!("{}/api", super::api_root(base_url)));
    Ok(Some(Discovery {
        issuer,
        authorization_endpoint,
        token_endpoint,
        registration_endpoint: registration,
        revocation_endpoint: s("revocation_endpoint"),
        resource,
    }))
}

// -------------------------------------------------------------- client

/// A registered client, kept per blyg (its id isn't a secret).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Client {
    pub issuer: String,
    pub client_id: String,
    pub redirect_uris: Vec<String>,
}

/// Register Burrow as a public native client with the loopback redirects.
pub fn register(d: &Discovery) -> Result<Client> {
    let redirect_uris = registered_redirect_uris();
    let body = json!({
        "client_name": CLIENT_NAME,
        "application_type": "native",
        "redirect_uris": redirect_uris,
        "token_endpoint_auth_method": "none",
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "scope": SCOPE,
    });
    let v: Value = match agent().post(&d.registration_endpoint).send_json(body) {
        Ok(r) => r
            .into_json()
            .map_err(|e| CoreError::Other(format!("registering Burrow: {e}")))?,
        Err(ureq::Error::Status(429, _)) => {
            return Err(CoreError::Rejected {
                status: 429,
                message:
                    "The blyg isn't taking new sign-ins right now. Try again in a few minutes."
                        .into(),
                details: vec![],
            });
        }
        Err(ureq::Error::Status(code, r)) => {
            return Err(CoreError::Rejected {
                status: code,
                message: format!(
                    "The blyg refused to register Burrow ({code}: {})",
                    oauth_error(r)
                ),
                details: vec![],
            });
        }
        Err(ureq::Error::Transport(_)) => return Err(CoreError::Offline),
    };
    let client_id = v
        .get("client_id")
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::Other("the blyg registered Burrow without a client id".into()))?;
    Ok(Client {
        issuer: d.issuer.clone(),
        client_id: client_id.to_string(),
        redirect_uris,
    })
}

/// Whether the server still knows `client` with `redirect`: an unattended
/// `prompt=none` authorize (no cookies) comes back to the redirect with
/// `login_required`; an unknown client goes to the server's error page.
pub fn client_is_known(d: &Discovery, client: &Client, redirect: &str) -> Result<bool> {
    let (_, challenge) = pkce();
    let url = authorize_url(
        d,
        &client.client_id,
        redirect,
        &challenge,
        "probe",
        Some("none"),
    )?;
    match agent().get(&url).call() {
        Ok(r) => Ok(r
            .header("location")
            .is_some_and(|l| l.starts_with(redirect))),
        Err(ureq::Error::Status(_, _)) => Ok(false),
        Err(ureq::Error::Transport(_)) => Err(CoreError::Offline),
    }
}

/// The kept client for this blyg, else a new registration (kept).
pub fn client_for(store: &dyn TokenStore, base_url: &str, d: &Discovery) -> Result<Client> {
    let key = client_key(base_url);
    let kept = store
        .get(&key)?
        .and_then(|s| serde_json::from_str::<Client>(&s).ok())
        .filter(|c| c.issuer == d.issuer && c.redirect_uris == registered_redirect_uris());
    if let Some(c) = kept
        && client_is_known(d, &c, &c.redirect_uris[0])?
    {
        return Ok(c);
    }
    let c = register(d)?;
    let text = serde_json::to_string(&c).map_err(|e| CoreError::Other(e.to_string()))?;
    store.set(&key, &text)?;
    Ok(c)
}

/// The authorize URL: code flow, S256 PKCE, state, every owner scope and
/// a refresh token, bound to the API resource.
pub fn authorize_url(
    d: &Discovery,
    client_id: &str,
    redirect: &str,
    challenge: &str,
    state: &str,
    prompt: Option<&str>,
) -> Result<String> {
    let mut url = url::Url::parse(&d.authorization_endpoint)
        .map_err(|e| CoreError::Other(format!("the blyg's authorize URL: {e}")))?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("response_type", "code")
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", redirect)
            .append_pair("scope", SCOPE)
            .append_pair("state", state)
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("resource", &d.resource);
        if let Some(p) = prompt {
            q.append_pair("prompt", p);
        }
    }
    Ok(url.to_string())
}

/// An OAuth error body's `error` (and description), for messages.
fn oauth_error(r: ureq::Response) -> String {
    let v: Value = r.into_json().unwrap_or(Value::Null);
    let e = v.get("error").and_then(Value::as_str).unwrap_or("error");
    match v.get("error_description").and_then(Value::as_str) {
        Some(d) => format!("{e}: {d}"),
        None => e.to_string(),
    }
}

// -------------------------------------------------------------- grant

/// A browser sign-in's tokens and where to renew and revoke them. Kept as
/// JSON in the Keychain; never logged.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthGrant {
    pub access: String,
    #[serde(default)]
    pub refresh: Option<String>,
    /// When the access token expires (Unix seconds).
    pub expires: u64,
    /// When the grant itself ends (the `grantExpires` claim: at most 30
    /// days after the owner's studio session began); 0 when not said.
    #[serde(default)]
    pub deadline: u64,
    /// The scopes granted (space-separated).
    #[serde(default)]
    pub scope: String,
    pub client_id: String,
    pub token_endpoint: String,
    #[serde(default)]
    pub revocation_endpoint: Option<String>,
    pub resource: String,
    /// The server refused to renew it, or its deadline passed: sign in again.
    #[serde(default)]
    pub invalid: bool,
}

impl std::fmt::Debug for OAuthGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthGrant")
            .field("expires", &self.expires)
            .field("deadline", &self.deadline)
            .field("scope", &self.scope)
            .field("invalid", &self.invalid)
            .finish_non_exhaustive()
    }
}

impl OAuthGrant {
    /// Past its deadline, or refused: only a new sign-in helps.
    pub fn ended(&self) -> bool {
        self.invalid || (self.deadline != 0 && self.deadline <= now_secs())
    }

    fn stale(&self) -> bool {
        self.expires <= now_secs() + RENEW_MARGIN
    }

    /// A token endpoint's answer → a grant (keeping the refresh token when
    /// a renewal doesn't rotate it).
    fn from_response(v: &Value, base: &GrantBase, old_refresh: Option<&str>) -> Result<Self> {
        let access = v
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CoreError::Other("the blyg's token answer had no access token".into())
            })?;
        let expires_in = v.get("expires_in").and_then(Value::as_u64).unwrap_or(3600);
        Ok(OAuthGrant {
            access: access.to_string(),
            refresh: v
                .get("refresh_token")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| old_refresh.map(str::to_string)),
            expires: now_secs() + expires_in,
            deadline: jwt_claim_u64(access, "grantExpires").unwrap_or(0),
            scope: v
                .get("scope")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            client_id: base.client_id.clone(),
            token_endpoint: base.token_endpoint.clone(),
            revocation_endpoint: base.revocation_endpoint.clone(),
            resource: base.resource.clone(),
            invalid: false,
        })
    }

    fn base(&self) -> GrantBase {
        GrantBase {
            client_id: self.client_id.clone(),
            token_endpoint: self.token_endpoint.clone(),
            revocation_endpoint: self.revocation_endpoint.clone(),
            resource: self.resource.clone(),
        }
    }
}

/// What a grant is renewed and revoked against.
struct GrantBase {
    client_id: String,
    token_endpoint: String,
    revocation_endpoint: Option<String>,
    resource: String,
}

/// POST a token request. A refused grant (`invalid_grant`, or the client
/// gone) is `Unauthorized`.
fn token_request(endpoint: &str, form: &[(&str, &str)]) -> Result<Value> {
    match agent().post(endpoint).send_form(form) {
        Ok(r) => r
            .into_json()
            .map_err(|e| CoreError::Other(format!("the blyg's token answer: {e}"))),
        Err(ureq::Error::Status(400 | 401, _)) => Err(CoreError::Unauthorized),
        Err(ureq::Error::Status(code, r)) => Err(CoreError::Rejected {
            status: code,
            message: format!("signing in failed ({code}: {})", oauth_error(r)),
            details: vec![],
        }),
        Err(ureq::Error::Transport(_)) => Err(CoreError::Offline),
    }
}

/// Renew with the refresh token (rotating it).
pub fn refresh(grant: &OAuthGrant) -> Result<OAuthGrant> {
    let Some(rt) = grant.refresh.as_deref() else {
        return Err(CoreError::Unauthorized);
    };
    let v = token_request(
        &grant.token_endpoint,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", rt),
            ("client_id", &grant.client_id),
            ("resource", &grant.resource),
        ],
    )?;
    OAuthGrant::from_response(&v, &grant.base(), Some(rt))
}

/// Revoke the grant's refresh token (RFC 7009), so it can't be renewed.
/// The current access token stays good until it expires (at most an hour).
pub fn revoke(grant: &OAuthGrant) -> Result<()> {
    let (Some(endpoint), Some(rt)) = (&grant.revocation_endpoint, &grant.refresh) else {
        return Ok(());
    };
    match agent().post(endpoint).send_form(&[
        ("token", rt),
        ("token_type_hint", "refresh_token"),
        ("client_id", &grant.client_id),
    ]) {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(code, r)) => Err(CoreError::Rejected {
            status: code,
            message: format!("revoking the sign-in failed ({code}: {})", oauth_error(r)),
            details: vec![],
        }),
        Err(ureq::Error::Transport(_)) => Err(CoreError::Offline),
    }
}

// ------------------------------------------------------------- session

/// One renewal at a time in this process: two `Api`s renewing with the
/// same refresh token would revoke the grant.
static RENEWING: Mutex<()> = Mutex::new(());

/// A grant in use: hands out a fresh access token, renewing (and saving)
/// it when due. Clones share the same grant.
#[derive(Clone)]
pub struct OAuthSession(Arc<Shared>);

struct Shared {
    grant: Mutex<OAuthGrant>,
    /// Where renewals are saved: the store and the grant's key.
    kept: Option<(Arc<dyn TokenStore>, String)>,
}

impl std::fmt::Debug for OAuthSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OAuth(<redacted>)")
    }
}

impl PartialEq for OAuthSession {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || self.grant() == other.grant()
    }
}

impl Eq for OAuthSession {}

impl OAuthSession {
    /// A grant that isn't kept anywhere (checking a new sign-in).
    pub fn new(grant: OAuthGrant) -> Self {
        OAuthSession(Arc::new(Shared {
            grant: Mutex::new(grant),
            kept: None,
        }))
    }

    /// A grant kept in `store` for `base_url`: renewals are saved there,
    /// and a renewal another `Api` saved first is picked up from there.
    pub fn kept(grant: OAuthGrant, store: Arc<dyn TokenStore>, base_url: &str) -> Self {
        OAuthSession(Arc::new(Shared {
            grant: Mutex::new(grant),
            kept: Some((store, grant_key(base_url))),
        }))
    }

    /// The grant as it stands.
    pub fn grant(&self) -> OAuthGrant {
        self.0.grant.lock().unwrap().clone()
    }

    /// Whether only a new browser sign-in will do.
    pub fn ended(&self) -> bool {
        self.0.grant.lock().unwrap().ended()
    }

    /// An access token to send, renewed first when it's about to expire.
    pub fn access_token(&self) -> Result<String> {
        {
            let g = self.0.grant.lock().unwrap();
            if g.ended() {
                return Err(CoreError::Unauthorized);
            }
            if !g.stale() {
                return Ok(g.access.clone());
            }
        }
        self.renew(None)
    }

    /// The server answered 401 to `rejected`: renew (unless another call
    /// already did) and return the token to retry with.
    pub fn after_unauthorized(&self, rejected: Option<&str>) -> Result<String> {
        self.renew(Some(rejected.unwrap_or_default()))
    }

    /// Renew under the process-wide lock. `rejected`: renew unless the
    /// current token differs from it; `None`: renew if it's stale.
    fn renew(&self, rejected: Option<&str>) -> Result<String> {
        let _one = RENEWING.lock().unwrap_or_else(|e| e.into_inner());
        let mut g = self.0.grant.lock().unwrap();
        // Another Api (or launch) may have renewed it: the kept copy wins.
        if let Some((store, key)) = &self.0.kept {
            match store.get(key)? {
                Some(text) => {
                    if let Ok(kept) = serde_json::from_str::<OAuthGrant>(&text)
                        && kept.client_id == g.client_id
                    {
                        *g = kept;
                    }
                }
                // Signed out meanwhile.
                None => return Err(CoreError::Unauthorized),
            }
        }
        if g.ended() {
            if !g.invalid {
                g.invalid = true;
                self.save(&g);
            }
            return Err(CoreError::Unauthorized);
        }
        let due = match rejected {
            Some(r) => g.access == r || g.stale(),
            None => g.stale(),
        };
        if !due {
            return Ok(g.access.clone());
        }
        match refresh(&g) {
            Ok(fresh) => {
                *g = fresh;
                self.save(&g);
                Ok(g.access.clone())
            }
            Err(CoreError::Unauthorized) => {
                g.invalid = true;
                self.save(&g);
                Err(CoreError::Unauthorized)
            }
            Err(e) => Err(e),
        }
    }

    fn save(&self, g: &OAuthGrant) {
        if let Some((store, key)) = &self.0.kept
            && let Ok(text) = serde_json::to_string(g)
        {
            // A failed save leaves the old refresh token kept; the next
            // launch then has to sign in again, which is the safe outcome.
            let _ = store.set(key, &text);
        }
    }
}

// ------------------------------------------------------------ loopback

/// The callback listener on 127.0.0.1.
#[derive(Debug)]
pub struct Loopback {
    listener: TcpListener,
    pub redirect_uri: String,
    state: String,
}

impl Loopback {
    /// Listen on the first free port of `ports`, else on any free port.
    pub fn bind(ports: &[u16], state: String) -> Result<Loopback> {
        let listener = ports
            .iter()
            .find_map(|&p| TcpListener::bind(("127.0.0.1", p)).ok())
            .or_else(|| TcpListener::bind(("127.0.0.1", 0)).ok())
            .ok_or_else(|| CoreError::Other("couldn't listen for the browser's answer".into()))?;
        let port = listener
            .local_addr()
            .map_err(|e| CoreError::Other(e.to_string()))?
            .port();
        Ok(Loopback {
            listener,
            redirect_uri: redirect_uri(port),
            state,
        })
    }

    /// Wait for `GET /oauth/callback?code=…&state=…`; returns the code.
    pub fn wait(&self, cancel: &AtomicBool, timeout: Duration) -> Result<String> {
        self.listener
            .set_nonblocking(true)
            .map_err(|e| CoreError::Other(e.to_string()))?;
        let deadline = Instant::now() + timeout;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(CoreError::Other("Sign-in cancelled".into()));
            }
            if Instant::now() > deadline {
                return Err(CoreError::Other(
                    "The browser sign-in timed out. Try again.".into(),
                ));
            }
            match self.listener.accept() {
                Ok((stream, _)) => {
                    if let Some(r) = self.handle_callback(stream) {
                        return r;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(CoreError::Other(e.to_string())),
            }
        }
    }

    /// Answer one request on the listener. `None`: not the callback (a
    /// stray request, or a wrong `state`), keep waiting.
    fn handle_callback(&self, mut stream: TcpStream) -> Option<Result<String>> {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).ok()?;
        let target = line.split_whitespace().nth(1).unwrap_or("/");
        let url = url::Url::parse(&format!("http://127.0.0.1{target}")).ok();
        let q = |k: &str| {
            url.as_ref().and_then(|u| {
                u.query_pairs()
                    .find(|(n, _)| n == k)
                    .map(|(_, v)| v.into_owned())
            })
        };
        let (status, msg, out) = match &url {
            Some(u) if u.path() != CALLBACK_PATH => ("404 Not Found", "Not found.", None),
            _ if q("state").as_deref() != Some(self.state.as_str()) => (
                "400 Bad Request",
                "This sign-in doesn't match the one Burrow started.",
                None,
            ),
            _ => match (q("code"), q("error")) {
                (Some(c), _) => (
                    "200 OK",
                    "Burrow is signed in to your blyg. You can close this tab.",
                    Some(Ok(c)),
                ),
                (None, Some(e)) if e == "access_denied" => (
                    "200 OK",
                    "You didn't allow Burrow. You can close this tab.",
                    Some(Err(CoreError::Other(
                        "You didn't allow Burrow in the browser.".into(),
                    ))),
                ),
                (None, e) => (
                    "400 Bad Request",
                    "The sign-in failed. Return to Burrow.",
                    Some(Err(CoreError::Other(format!(
                        "The blyg's sign-in failed ({}).",
                        e.unwrap_or_else(|| "no code".into())
                    )))),
                ),
            },
        };
        let body = format!(
            "<!doctype html><meta charset=utf-8><title>Burrow</title><p style=\"font:16px system-ui;margin:3em\">{msg}</p>"
        );
        let _ = write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{body}",
            body.len()
        );
        out
    }
}

// --------------------------------------------------------------- flow

/// A browser sign-in in progress: open [`BrowserSignIn::authorize_url`],
/// then [`BrowserSignIn::finish`].
#[derive(Debug)]
pub struct BrowserSignIn {
    pub authorize_url: String,
    discovery: Discovery,
    client_id: String,
    verifier: String,
    loopback: Loopback,
}

impl BrowserSignIn {
    /// Discover, find or register the client, and start listening. `None`
    /// when the blyg has no browser sign-in (studio older than 0.28).
    pub fn start(base_url: &str, store: &dyn TokenStore) -> Result<Option<BrowserSignIn>> {
        let Some(d) = discover(base_url)? else {
            return Ok(None);
        };
        let client = client_for(store, base_url, &d)?;
        let state = random_state();
        let loopback = Loopback::bind(&LOOPBACK_PORTS, state.clone())?;
        let (verifier, challenge) = pkce();
        let authorize_url = authorize_url(
            &d,
            &client.client_id,
            &loopback.redirect_uri,
            &challenge,
            &state,
            None,
        )?;
        Ok(Some(BrowserSignIn {
            authorize_url,
            discovery: d,
            client_id: client.client_id,
            verifier,
            loopback,
        }))
    }

    /// Wait for the browser to come back, then exchange the code.
    pub fn finish(&self, cancel: &AtomicBool, timeout: Duration) -> Result<OAuthGrant> {
        let code = self.loopback.wait(cancel, timeout)?;
        self.exchange(&code)
    }

    fn exchange(&self, code: &str) -> Result<OAuthGrant> {
        let d = &self.discovery;
        let v = token_request(
            &d.token_endpoint,
            &[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("client_id", &self.client_id),
                ("redirect_uri", &self.loopback.redirect_uri),
                ("code_verifier", &self.verifier),
                ("resource", &d.resource),
            ],
        )
        .map_err(|e| match e {
            CoreError::Unauthorized => {
                CoreError::Other("The blyg refused the sign-in code. Try again.".into())
            }
            e => e,
        })?;
        let base = GrantBase {
            client_id: self.client_id.clone(),
            token_endpoint: d.token_endpoint.clone(),
            revocation_endpoint: d.revocation_endpoint.clone(),
            resource: d.resource.clone(),
        };
        OAuthGrant::from_response(&v, &base, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_is_s256_of_the_verifier() {
        let (v, c) = pkce();
        assert_eq!(v.len(), 43);
        assert!(
            v.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        assert_eq!(c, s256(&v));
        // Checked against an independent SHA-256 + base64url.
        assert_eq!(
            s256("dBjftJeZ4CVP-mJ92K1qUZtqF3tMXr2W1T3B6O1gRAo"),
            "hl96zEcOFcmUqxbFo_jlJswmKFmgIezLSFm7QDSN60U"
        );
        assert_ne!(pkce().0, v, "fresh each time");
        assert_ne!(random_state(), random_state());
    }

    #[test]
    fn authorize_url_asks_for_everything() {
        let d = Discovery {
            issuer: "https://blyg.example.com/studio/auth".into(),
            authorization_endpoint: "https://blyg.example.com/studio/auth/oauth2/authorize".into(),
            token_endpoint: "https://blyg.example.com/studio/auth/oauth2/token".into(),
            registration_endpoint: "https://blyg.example.com/studio/auth/oauth2/register".into(),
            revocation_endpoint: None,
            resource: "https://blyg.example.com/api".into(),
        };
        let u = authorize_url(&d, "cid", &redirect_uri(47811), "chal", "st", None).unwrap();
        let u = url::Url::parse(&u).unwrap();
        assert_eq!(u.path(), "/studio/auth/oauth2/authorize");
        let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["client_id"], "cid");
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:47811/oauth/callback");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["code_challenge"], "chal");
        assert_eq!(q["state"], "st");
        assert_eq!(q["resource"], "https://blyg.example.com/api");
        assert_eq!(
            q["scope"],
            "owner:read owner:draft owner:publish owner:manage offline_access"
        );
        assert!(!q.contains_key("prompt"));
        assert_eq!(
            discovery_url("https://blyg.example.com/blyg/"),
            "https://blyg.example.com/blyg/studio/auth/.well-known/openid-configuration"
        );
    }

    #[test]
    fn reads_an_insufficient_scope_challenge() {
        let h = r#"Bearer resource_metadata="https://blyg.example.com/studio/auth/resources/api", scope="owner:draft owner:publish", error="insufficient_scope""#;
        assert_eq!(
            insufficient_scope(h),
            Some(vec!["owner:draft".to_string(), "owner:publish".to_string()])
        );
        assert_eq!(
            insufficient_scope(r#"Bearer resource_metadata="x", scope="owner:read""#),
            None
        );
        let m = missing_scope_message(
            &["owner:draft".into(), "owner:publish".into()],
            Some("owner:read owner:draft"),
        );
        assert!(m.contains("the owner:publish permission."), "{m}");
        assert!(!m.contains("owner:draft"), "{m}");
        let m = missing_scope_message(&["owner:publish".into()], None);
        assert!(m.contains("Studio → More → Client access"), "{m}");
        assert!(m.contains("owner:publish"), "{m}");
    }

    #[test]
    fn grant_debug_never_shows_a_token() {
        let g = OAuthGrant {
            access: "acc-secret".into(),
            refresh: Some("ref-secret".into()),
            expires: 1,
            deadline: 2,
            scope: "owner:read".into(),
            client_id: "cid".into(),
            token_endpoint: "https://blyg.example.com/t".into(),
            revocation_endpoint: None,
            resource: "https://blyg.example.com/api".into(),
            invalid: false,
        };
        let s = format!("{g:?} {:?}", OAuthSession::new(g.clone()));
        assert!(!s.contains("secret"), "{s}");
        assert!(g.ended(), "a deadline in 1970 has passed");
    }

    #[test]
    fn reads_a_jwt_claim() {
        let payload = URL_SAFE_NO_PAD.encode(br#"{"grantExpires":1893456000,"sub":"owner"}"#);
        let t = format!("h.{payload}.s");
        assert_eq!(jwt_claim_u64(&t, "grantExpires"), Some(1893456000));
        assert_eq!(jwt_claim_u64(&t, "nope"), None);
        assert_eq!(jwt_claim_u64("not a jwt", "grantExpires"), None);
    }
}
