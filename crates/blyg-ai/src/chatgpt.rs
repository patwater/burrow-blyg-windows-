//! "Sign in with ChatGPT": OpenAI's Codex OAuth client plus the ChatGPT
//! Codex Responses endpoint, done the way the pi coding agent does it.
//!
//! **Unofficial.** OpenAI has no sanctioned route for third-party apps to use
//! a ChatGPT plan; this borrows the Codex CLI's public OAuth client and may
//! break or be blocked at any time. See docs/AI.md.
//!
//! Every constant below is copied from pi-ai 0.85.1
//! (`@earendil-works/pi-ai`, github.com/badlogic/pi-mono `packages/ai`):
//! - `dist/auth/oauth/openai-codex.js`: client id, auth URLs, scope, the
//!   authorize query, the device-code endpoints and the JWT claim path;
//! - `dist/auth/oauth/pkce.js`: 32 random bytes, base64url, S256;
//! - `dist/api/openai-codex-responses.js`: base URL, `/codex/responses`, the
//!   headers and the request body (`store: false`, `stream: true`, …).

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD};
use blyg_core::config::TokenStore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::error::{AiError, Result};
use crate::http;
use crate::provider::{CancelFlag, GenRequest, GenResult, ModelInfo, Provider, ProviderKind};

pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const AUTH_BASE_URL: &str = "https://auth.openai.com";
pub const SCOPE: &str = "openid profile email offline_access";
/// The Codex client's registered redirect is `http://localhost:1455/auth/callback`.
pub const CALLBACK_PORT: u16 = 1455;
pub const CALLBACK_PATH: &str = "/auth/callback";
pub const DEVICE_VERIFICATION_PATH: &str = "/codex/device";
pub const DEVICE_REDIRECT_PATH: &str = "/deviceauth/callback";
pub const DEVICE_CODE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
pub const JWT_CLAIM_PATH: &str = "https://api.openai.com/auth";
pub const CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api";
/// pi sends `originator: pi`; we identify ourselves instead.
pub const ORIGINATOR: &str = "blygger";
/// The flagship (OpenAI docs), also in pi's openai-codex catalog.
pub const DEFAULT_MODEL: &str = "gpt-6-astra";
/// Keychain account (via `TokenStore`) for the OAuth credentials JSON.
pub const KEYCHAIN_ACCOUNT: &str = "blygger-ai.chatgpt-oauth";

/// pi's openai-codex model catalog (`providers/data/openai-codex.json`).
pub const CATALOG: &[&str] = &[
    "gpt-6-astra",
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-5.5",
    "gpt-5.4",
    "gpt-5.4-mini",
    "gpt-5.3-codex-spark",
];

/// Stored OAuth credentials (JSON in the Keychain). Never logged.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    pub access: String,
    pub refresh: String,
    /// Access-token expiry, ms since the epoch.
    pub expires: u64,
    pub account_id: String,
    /// Set when a refresh was rejected: the user must sign in again.
    #[serde(default)]
    pub invalid: bool,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("expires", &self.expires)
            .field("invalid", &self.invalid)
            .finish_non_exhaustive()
    }
}

impl Credentials {
    pub fn load(store: &dyn TokenStore) -> Result<Option<Credentials>> {
        Ok(store
            .get(KEYCHAIN_ACCOUNT)?
            .and_then(|s| serde_json::from_str(&s).ok()))
    }
    pub fn save(&self, store: &dyn TokenStore) -> Result<()> {
        let s = serde_json::to_string(self).map_err(|e| AiError::Storage(e.to_string()))?;
        Ok(store.set(KEYCHAIN_ACCOUNT, &s)?)
    }
    pub fn expires_soon(&self) -> bool {
        self.expires <= now_ms() + 60_000
    }
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::getrandom(&mut b).expect("OS randomness");
    b
}

/// PKCE verifier + S256 challenge, as pi's `generatePKCE`.
pub fn pkce() -> (String, String) {
    let verifier = URL_SAFE_NO_PAD.encode(random_bytes::<32>());
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

fn random_state() -> String {
    random_bytes::<16>()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `chatgpt_account_id` from the access token's JWT payload.
pub fn account_id_from_jwt(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .or_else(|_| STANDARD_NO_PAD.decode(payload.trim_end_matches('=')))
        .ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v.get(JWT_CLAIM_PATH)?
        .get("chatgpt_account_id")?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// The OAuth half: browser PKCE with a localhost callback, device code, refresh.
#[derive(Debug, Clone)]
pub struct ChatGptOAuth {
    pub auth_base: String,
    pub client_id: String,
    /// Localhost callback port (1455 for real; tests use 0 = any free port).
    pub callback_port: u16,
    /// Floor for the device-code poll interval.
    pub min_poll_interval: Duration,
}

impl Default for ChatGptOAuth {
    fn default() -> Self {
        ChatGptOAuth {
            auth_base: AUTH_BASE_URL.to_string(),
            client_id: CLIENT_ID.to_string(),
            callback_port: CALLBACK_PORT,
            min_poll_interval: Duration::from_secs(1),
        }
    }
}

/// A started browser login: open `url`, then `wait()` for the callback (or
/// let the user paste the redirect URL / code into `parse_pasted`).
#[derive(Debug)]
pub struct BrowserLogin {
    pub url: String,
    pub redirect_uri: String,
    verifier: String,
    state: String,
    /// `None` when the port was busy: only the paste fallback works then.
    listener: Option<TcpListener>,
}

/// A started device-code login: show `user_code` and `verification_uri`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCode {
    pub device_auth_id: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: Duration,
}

impl ChatGptOAuth {
    fn token_url(&self) -> String {
        format!("{}/oauth/token", self.auth_base)
    }

    /// Bind the callback listener and build the authorize URL (pi's
    /// `createAuthorizationFlow`).
    pub fn start_browser_login(&self) -> Result<BrowserLogin> {
        let listener = TcpListener::bind(("127.0.0.1", self.callback_port)).ok();
        let port = listener
            .as_ref()
            .and_then(|l| l.local_addr().ok())
            .map(|a| a.port())
            .unwrap_or(self.callback_port);
        let redirect_uri = format!("http://localhost:{port}{CALLBACK_PATH}");
        let (verifier, challenge) = pkce();
        let state = random_state();
        let mut url = url::Url::parse(&format!("{}/oauth/authorize", self.auth_base))
            .map_err(|e| AiError::Provider(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &self.client_id)
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("scope", SCOPE)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state)
            .append_pair("id_token_add_organizations", "true")
            .append_pair("codex_cli_simplified_flow", "true")
            .append_pair("originator", ORIGINATOR);
        Ok(BrowserLogin {
            url: url.to_string(),
            redirect_uri,
            verifier,
            state,
            listener,
        })
    }

    /// Exchange an authorization code (pi's `exchangeAuthorizationCode`).
    pub fn exchange_code(
        &self,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<Credentials> {
        let res = http::agent().post(&self.token_url()).send_form(&[
            ("grant_type", "authorization_code"),
            ("client_id", &self.client_id),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", redirect_uri),
        ]);
        token_response(res, "exchange")
    }

    /// Complete a browser login: wait for the callback, then exchange.
    pub fn finish_browser_login(
        &self,
        login: &BrowserLogin,
        cancel: &CancelFlag,
        timeout: Duration,
    ) -> Result<Credentials> {
        let code = login.wait(cancel, timeout)?;
        self.exchange_code(&code, &login.verifier, &login.redirect_uri)
    }

    /// Complete a browser login from a pasted redirect URL or code.
    pub fn finish_with_pasted(&self, login: &BrowserLogin, input: &str) -> Result<Credentials> {
        let code = login.parse_pasted(input)?;
        self.exchange_code(&code, &login.verifier, &login.redirect_uri)
    }

    /// Refresh (pi's `refreshAccessToken`).
    pub fn refresh(&self, refresh_token: &str) -> Result<Credentials> {
        let res = http::agent().post(&self.token_url()).send_form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", &self.client_id),
        ]);
        token_response(res, "refresh")
    }

    /// Start the device-code flow (pi's `startOpenAICodexDeviceAuth`).
    pub fn start_device(&self) -> Result<DeviceCode> {
        let res = http::agent()
            .post(&format!(
                "{}/api/accounts/deviceauth/usercode",
                self.auth_base
            ))
            .send_json(json!({ "client_id": self.client_id }));
        let v = match res {
            Err(ureq::Error::Status(404, _)) => {
                return Err(AiError::Provider(
                    "device code login is not enabled for this server; use browser login".into(),
                ));
            }
            r => http::read_json("OpenAI auth", r)?,
        };
        let interval = match v.get("interval") {
            Some(Value::String(s)) => s.trim().parse::<f64>().ok(),
            Some(Value::Number(n)) => n.as_f64(),
            _ => None,
        };
        let (Some(id), Some(code), Some(interval)) = (
            v.get("device_auth_id").and_then(Value::as_str),
            v.get("user_code").and_then(Value::as_str),
            interval.filter(|i| i.is_finite() && *i >= 0.0),
        ) else {
            return Err(AiError::Provider("invalid device code response".into()));
        };
        Ok(DeviceCode {
            device_auth_id: id.to_string(),
            user_code: code.to_string(),
            verification_uri: format!("{}{DEVICE_VERIFICATION_PATH}", self.auth_base),
            interval: Duration::from_secs_f64(interval),
        })
    }

    /// Poll until the user approves, then exchange (pi's `pollOpenAICodexDeviceAuth`).
    pub fn poll_device(&self, device: &DeviceCode, cancel: &CancelFlag) -> Result<Credentials> {
        let deadline = Instant::now() + DEVICE_CODE_TIMEOUT;
        let mut interval = device.interval.max(self.min_poll_interval);
        while Instant::now() < deadline {
            cancel.check()?;
            let res = http::agent()
                .post(&format!("{}/api/accounts/deviceauth/token", self.auth_base))
                .send_json(json!({
                    "device_auth_id": device.device_auth_id,
                    "user_code": device.user_code,
                }));
            match res {
                Ok(r) => {
                    let v: Value = r.into_json().map_err(|e| {
                        AiError::Provider(format!("invalid device auth response: {e}"))
                    })?;
                    let (Some(code), Some(verifier)) = (
                        v.get("authorization_code").and_then(Value::as_str),
                        v.get("code_verifier").and_then(Value::as_str),
                    ) else {
                        return Err(AiError::Provider(
                            "invalid device auth token response".into(),
                        ));
                    };
                    let redirect = format!("{}{DEVICE_REDIRECT_PATH}", self.auth_base);
                    return self.exchange_code(code, verifier, &redirect);
                }
                Err(ureq::Error::Status(403 | 404, _)) => {}
                Err(ureq::Error::Status(code, r)) => {
                    let body = r.into_string().unwrap_or_default();
                    let err = serde_json::from_str::<Value>(&body).ok().and_then(|v| {
                        let e = v.get("error")?;
                        e.get("code")
                            .and_then(Value::as_str)
                            .or(e.as_str())
                            .map(str::to_string)
                    });
                    match err.as_deref() {
                        Some("deviceauth_authorization_pending") => {}
                        Some("slow_down") => interval += Duration::from_secs(5),
                        _ => {
                            return Err(AiError::Provider(format!(
                                "device auth failed with status {code}: {}",
                                http::error_message(&body)
                            )));
                        }
                    }
                }
                Err(e) => return Err(http::map_err("OpenAI auth", e)),
            }
            sleep_cancellable(interval, cancel)?;
        }
        Err(AiError::Provider("device flow timed out".into()))
    }
}

fn sleep_cancellable(d: Duration, cancel: &CancelFlag) -> Result<()> {
    let end = Instant::now() + d;
    while Instant::now() < end {
        cancel.check()?;
        std::thread::sleep((end - Instant::now()).min(Duration::from_millis(50)));
    }
    Ok(())
}

fn token_response(
    res: std::result::Result<ureq::Response, ureq::Error>,
    op: &str,
) -> Result<Credentials> {
    let v = match res {
        Ok(r) => r
            .into_json::<Value>()
            .map_err(|e| AiError::Provider(format!("token {op}: invalid JSON: {e}")))?,
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            let msg = format!("token {op} failed ({code}): {}", http::error_message(&body));
            // A rejected refresh means the sign-in is dead.
            return Err(if op == "refresh" && (400..500).contains(&code) {
                AiError::TokenExpired(format!("ChatGPT ({msg})"))
            } else {
                AiError::Provider(msg)
            });
        }
        Err(e) => return Err(http::map_err("OpenAI auth", e)),
    };
    let (Some(access), Some(refresh), Some(expires_in)) = (
        v.get("access_token").and_then(Value::as_str),
        v.get("refresh_token").and_then(Value::as_str),
        v.get("expires_in").and_then(Value::as_f64),
    ) else {
        return Err(AiError::Provider(format!(
            "token {op} response missing fields"
        )));
    };
    let account_id = account_id_from_jwt(access)
        .ok_or_else(|| AiError::Provider("failed to extract accountId from token".into()))?;
    Ok(Credentials {
        access: access.to_string(),
        refresh: refresh.to_string(),
        expires: now_ms() + (expires_in * 1000.0) as u64,
        account_id,
        invalid: false,
    })
}

impl BrowserLogin {
    /// Wait for `GET /auth/callback?code=…&state=…` on the listener.
    pub fn wait(&self, cancel: &CancelFlag, timeout: Duration) -> Result<String> {
        let Some(listener) = &self.listener else {
            return Err(AiError::Provider(format!(
                "port {CALLBACK_PORT} is busy; paste the redirect URL instead"
            )));
        };
        listener
            .set_nonblocking(true)
            .map_err(|e| AiError::Provider(e.to_string()))?;
        let deadline = Instant::now() + timeout;
        loop {
            cancel.check()?;
            if Instant::now() > deadline {
                return Err(AiError::Provider("sign-in timed out".into()));
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    if let Some(code) = self.handle_callback(stream) {
                        return Ok(code);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(AiError::Provider(e.to_string())),
            }
        }
    }

    fn handle_callback(&self, mut stream: TcpStream) -> Option<String> {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).ok()?;
        let target = line.split_whitespace().nth(1).unwrap_or("/");
        let url = url::Url::parse(&format!("http://localhost{target}")).ok();
        let q = |k: &str| {
            url.as_ref().and_then(|u| {
                u.query_pairs()
                    .find(|(n, _)| n == k)
                    .map(|(_, v)| v.into_owned())
            })
        };
        let (status, msg, code) = match &url {
            Some(u) if u.path() != CALLBACK_PATH => {
                ("404 Not Found", "Callback route not found.", None)
            }
            _ if q("state").as_deref() != Some(self.state.as_str()) => {
                ("400 Bad Request", "State mismatch.", None)
            }
            _ => match q("code") {
                Some(c) => (
                    "200 OK",
                    "OpenAI authentication completed. You can close this window and return to Blygger.",
                    Some(c),
                ),
                None => ("400 Bad Request", "Missing authorization code.", None),
            },
        };
        let body = format!(
            "<!doctype html><meta charset=utf-8><title>Blygger</title><p style=\"font:16px system-ui;margin:3em\">{msg}</p>"
        );
        let _ = write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        code
    }

    /// Accept a pasted redirect URL, `code#state`, a query string, or a bare
    /// code (pi's `parseAuthorizationInput`).
    pub fn parse_pasted(&self, input: &str) -> Result<String> {
        let v = input.trim();
        let (code, state) = if let Ok(u) = url::Url::parse(v) {
            let get = |k: &str| {
                u.query_pairs()
                    .find(|(n, _)| n == k)
                    .map(|(_, v)| v.into_owned())
            };
            (get("code"), get("state"))
        } else if let Some((c, s)) = v.split_once('#') {
            (Some(c.to_string()), Some(s.to_string()))
        } else if v.contains("code=") {
            let pairs: Vec<(String, String)> = url::form_urlencoded::parse(v.as_bytes())
                .into_owned()
                .collect();
            let get = |k: &str| pairs.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
            (get("code"), get("state"))
        } else {
            (Some(v.to_string()).filter(|s| !s.is_empty()), None)
        };
        if state.as_deref().is_some_and(|s| s != self.state) {
            return Err(AiError::Provider("state mismatch".into()));
        }
        code.filter(|c| !c.is_empty())
            .ok_or_else(|| AiError::Provider("missing authorization code".into()))
    }
}

/// The provider: ChatGPT Codex Responses endpoint with stored OAuth tokens.
pub struct ChatGptAccount {
    store: Arc<dyn TokenStore>,
    oauth: ChatGptOAuth,
    base_url: String,
    model: String,
}

impl ChatGptAccount {
    pub fn new(store: Arc<dyn TokenStore>) -> Self {
        ChatGptAccount {
            store,
            oauth: ChatGptOAuth::default(),
            base_url: CODEX_BASE_URL.to_string(),
            model: DEFAULT_MODEL.to_string(),
        }
    }
    pub fn with_oauth(mut self, oauth: ChatGptOAuth) -> Self {
        self.oauth = oauth;
        self
    }
    pub fn with_base_url(mut self, base: impl Into<String>) -> Self {
        self.base_url = base.into().trim_end_matches('/').to_string();
        self
    }
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// pi's `resolveCodexUrl`.
    fn url(&self) -> String {
        let b = &self.base_url;
        if b.ends_with("/codex/responses") {
            b.clone()
        } else if b.ends_with("/codex") {
            format!("{b}/responses")
        } else {
            format!("{b}/codex/responses")
        }
    }

    pub fn body(&self, req: &GenRequest) -> Value {
        json!({
            "model": req.model.clone().unwrap_or_else(|| self.model.clone()),
            "store": false,
            "stream": true,
            "instructions": req.system,
            "input": [{
                "role": "user",
                "content": [{ "type": "input_text", "text": req.user }],
            }],
            "text": { "verbosity": "medium" },
            "include": ["reasoning.encrypted_content"],
            "tool_choice": "auto",
            "parallel_tool_calls": true,
        })
    }

    fn credentials(&self) -> Result<Credentials> {
        let c = Credentials::load(self.store.as_ref())?
            .ok_or_else(|| AiError::NotConfigured("not signed in to ChatGPT".into()))?;
        if c.invalid {
            return Err(AiError::TokenExpired("ChatGPT".into()));
        }
        Ok(c)
    }

    /// Refresh and persist; a rejected refresh marks the stored creds invalid.
    fn refresh(&self, old: &Credentials) -> Result<Credentials> {
        match self.oauth.refresh(&old.refresh) {
            Ok(c) => {
                c.save(self.store.as_ref())?;
                Ok(c)
            }
            Err(e @ AiError::TokenExpired(_)) => {
                let mut dead = old.clone();
                dead.invalid = true;
                dead.save(self.store.as_ref())?;
                Err(e)
            }
            Err(e) => Err(e),
        }
    }

    #[allow(clippy::result_large_err)] // ureq 2's own error type
    fn post(
        &self,
        creds: &Credentials,
        body: &Value,
    ) -> std::result::Result<ureq::Response, ureq::Error> {
        http::agent()
            .post(&self.url())
            .set("authorization", &format!("Bearer {}", creds.access))
            .set("chatgpt-account-id", &creds.account_id)
            .set("originator", ORIGINATOR)
            .set(
                "user-agent",
                &format!(
                    "blygger ({} {})",
                    std::env::consts::OS,
                    std::env::consts::ARCH
                ),
            )
            .set("openai-beta", "responses=experimental")
            .set("accept", "text/event-stream")
            .set("content-type", "application/json")
            .send_json(body.clone())
    }
}

impl Provider for ChatGptAccount {
    fn kind(&self) -> ProviderKind {
        ProviderKind::ChatgptAccount
    }

    fn generate(&self, req: GenRequest, on_delta: &mut dyn FnMut(&str)) -> Result<GenResult> {
        req.cancel.check()?;
        let mut creds = self.credentials()?;
        let mut refreshed = false;
        if creds.expires_soon() {
            creds = self.refresh(&creds)?;
            refreshed = true;
        }
        let body = self.body(&req);
        let res = match self.post(&creds, &body) {
            Err(ureq::Error::Status(401, _)) if !refreshed => {
                creds = self.refresh(&creds)?;
                self.post(&creds, &body)
            }
            r => r,
        };
        let res = res.map_err(|e| match e {
            ureq::Error::Status(401 | 403, _) => AiError::Unauthorized {
                provider: "ChatGPT".into(),
            },
            ureq::Error::Status(code, r) => codex_error(code, &r.into_string().unwrap_or_default()),
            e => http::map_err("ChatGPT", e),
        })?;
        let model = req.model.clone().unwrap_or_else(|| self.model.clone());
        crate::openai::stream_responses(res.into_reader(), &req.cancel, on_delta, model)
    }

    fn list_models(&self) -> Result<Vec<ModelInfo>> {
        Ok(CATALOG
            .iter()
            .map(|id| ModelInfo {
                id: id.to_string(),
                display_name: None,
            })
            .collect())
    }
}

/// pi's `parseErrorResponse`: a friendly usage-limit message.
fn codex_error(status: u16, body: &str) -> AiError {
    let v: Option<Value> = serde_json::from_str(body).ok();
    let err = v.as_ref().and_then(|v| v.get("error"));
    let code = err
        .and_then(|e| e.get("code").or_else(|| e.get("type")))
        .and_then(Value::as_str)
        .unwrap_or("");
    let limited = status == 429
        || [
            "usage_limit_reached",
            "usage_not_included",
            "rate_limit_exceeded",
        ]
        .iter()
        .any(|c| code.contains(c));
    if limited {
        let plan = err
            .and_then(|e| e.get("plan_type"))
            .and_then(Value::as_str)
            .map(|p| format!(" ({} plan)", p.to_lowercase()))
            .unwrap_or_default();
        return AiError::Provider(format!("You have hit your ChatGPT usage limit{plan}."));
    }
    AiError::Provider(format!(
        "ChatGPT request failed: {status} {}",
        http::error_message(body)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_is_s256_of_verifier() {
        let (v, c) = pkce();
        assert_eq!(v.len(), 43);
        assert_eq!(c, URL_SAFE_NO_PAD.encode(Sha256::digest(v.as_bytes())));
        assert!(!v.contains('=') && !v.contains('+') && !v.contains('/'));
    }

    #[test]
    fn jwt_account_id() {
        let payload = URL_SAFE_NO_PAD
            .encode(br#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acc_1"}}"#);
        assert_eq!(
            account_id_from_jwt(&format!("h.{payload}.s")).as_deref(),
            Some("acc_1")
        );
        assert_eq!(account_id_from_jwt("nope"), None);
    }

    #[test]
    fn authorize_url_matches_pi() {
        let o = ChatGptOAuth {
            callback_port: 0,
            ..Default::default()
        };
        let l = o.start_browser_login().unwrap();
        let u = url::Url::parse(&l.url).unwrap();
        assert_eq!(
            u.as_str().split('?').next(),
            Some("https://auth.openai.com/oauth/authorize")
        );
        let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], CLIENT_ID);
        assert_eq!(q["scope"], SCOPE);
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["id_token_add_organizations"], "true");
        assert_eq!(q["codex_cli_simplified_flow"], "true");
        assert!(q["redirect_uri"].starts_with("http://localhost:"));
        assert!(q["redirect_uri"].ends_with("/auth/callback"));
    }

    #[test]
    fn pasted_input_forms() {
        let o = ChatGptOAuth {
            callback_port: 0,
            ..Default::default()
        };
        let l = o.start_browser_login().unwrap();
        let st = l.state.clone();
        assert_eq!(
            l.parse_pasted(&format!(
                "http://localhost:1455/auth/callback?code=abc&state={st}"
            ))
            .unwrap(),
            "abc"
        );
        assert_eq!(l.parse_pasted(&format!("abc#{st}")).unwrap(), "abc");
        assert_eq!(
            l.parse_pasted(&format!("code=abc&state={st}")).unwrap(),
            "abc"
        );
        assert_eq!(l.parse_pasted("  abc ").unwrap(), "abc");
        assert!(l.parse_pasted("abc#wrong").is_err());
        assert!(l.parse_pasted("").is_err());
    }
}
