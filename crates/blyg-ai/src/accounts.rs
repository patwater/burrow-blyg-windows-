//! Which AI providers are enabled, the default provider/model, and the
//! credentials behind them.
//!
//! Settings live in the app's one config file (blyg-core's
//! [`ConfigStore`]): `ai-provider`, `ai-model`, `ai-provider-model`,
//! `ai-enable`, `cloudflare-account-id`, `ai-style-prompt`. Never secrets.
//! Secrets go through blyg-core's `TokenStore` (the macOS Keychain in the
//! app) under their own accounts, distinct from the blyg owner token:
//!
//! | provider              | Keychain account                    |
//! |-----------------------|-------------------------------------|
//! | Anthropic API key     | `blygger-ai.anthropic-api-key`      |
//! | OpenAI API key        | `blygger-ai.openai-api-key`         |
//! | ChatGPT OAuth tokens  | `blygger-ai.chatgpt-oauth` (JSON)   |
//! | Cloudflare API token  | `blygger-ai.cloudflare-api-token`   |
//! | Blyg server           | the existing owner token (by base URL) |
//!
//! The "Sign in with ChatGPT is unofficial" notice is app state, not config:
//! `blyg_core::state::AppState::chatgpt_notice_shown`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use blyg_core::config::{Change, Config, ConfigStore, TokenStore};

use crate::anthropic::{self, AnthropicApi};
use crate::blyg_server::BlygServer;
use crate::chatgpt::{self, BrowserLogin, ChatGptAccount, ChatGptOAuth, Credentials, DeviceCode};
use crate::claude_code::{self, LocalClaudeCode};
use crate::cli::CliLocator;
use crate::cloudflare::{self, CloudflareWorkersAi};
use crate::codex::{self, LocalCodex};
use crate::error::{AiError, Result};
use crate::openai::{self, OpenAiApi};
use crate::provider::{CancelFlag, Provider, ProviderKind};

pub const ANTHROPIC_KEY_ACCOUNT: &str = "blygger-ai.anthropic-api-key";
pub const OPENAI_KEY_ACCOUNT: &str = "blygger-ai.openai-api-key";

/// When no default is set, the first ready provider in this order wins.
const PREFERENCE: [ProviderKind; 7] = [
    ProviderKind::LocalClaudeCode,
    ProviderKind::AnthropicApi,
    ProviderKind::ChatgptAccount,
    ProviderKind::OpenaiApi,
    ProviderKind::LocalCodex,
    ProviderKind::CloudflareWorkersAi,
    ProviderKind::BlygServer,
];

/// Whether `kind` is in `ai-enable` (by default the two local CLI bridges;
/// everything else is off until signed in).
pub fn enabled_in(config: &Config, kind: ProviderKind) -> bool {
    config.ai_enabled().iter().any(|e| e == kind.config_name())
}

/// The model for `kind`: `ai-model` when it's the `ai-provider`, else its
/// `ai-provider-model` entry, else the provider's default.
pub fn model_in(config: &Config, kind: ProviderKind) -> Option<String> {
    let name = kind.config_name();
    if config.ai_provider() == Some(name)
        && let Some(m) = config.ai_model()
    {
        return Some(m.to_string());
    }
    config
        .ai_provider_models()
        .into_iter()
        .rev()
        .find(|(p, _)| p == name)
        .map(|(_, m)| m)
        .or_else(|| kind.default_model().map(str::to_string))
}

/// Per-provider state for Settings › AI accounts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderStatus {
    /// Credentials present (API key / OAuth tokens / owner token).
    SignedIn,
    /// No credentials yet.
    NotSet,
    /// Local CLI found at this path.
    CliFound(PathBuf),
    CliNotFound,
    /// OAuth refresh was rejected: sign in again.
    TokenExpired,
}

impl ProviderStatus {
    pub fn is_ready(&self) -> bool {
        matches!(self, ProviderStatus::SignedIn | ProviderStatus::CliFound(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRow {
    pub kind: ProviderKind,
    pub enabled: bool,
    pub status: ProviderStatus,
    pub model: Option<String>,
    pub is_default: bool,
}

/// Credentials for `Accounts::sign_in`. ChatGPT uses its own browser /
/// device-code methods; the CLI bridges and the blyg server just switch on.
pub enum Credential {
    ApiKey(String),
    Cloudflare {
        account_id: String,
        api_token: String,
    },
    /// For the CLI bridges and the blyg server: nothing to store.
    None,
}

/// Service endpoints; overridable so tests talk to mock servers.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub anthropic: String,
    pub openai: String,
    pub chatgpt: String,
    pub chatgpt_oauth: ChatGptOAuth,
    pub cloudflare: String,
    pub cli: CliLocator,
    /// `None` = the config's `blyg-url`.
    pub blyg_base_url: Option<String>,
}

impl Default for Endpoints {
    fn default() -> Self {
        Endpoints {
            anthropic: anthropic::DEFAULT_BASE_URL.into(),
            openai: openai::DEFAULT_BASE_URL.into(),
            chatgpt: chatgpt::CODEX_BASE_URL.into(),
            chatgpt_oauth: ChatGptOAuth::default(),
            cloudflare: cloudflare::DEFAULT_BASE_URL.into(),
            cli: CliLocator::default(),
            blyg_base_url: None,
        }
    }
}

pub struct Accounts {
    config: ConfigStore,
    store: Arc<dyn TokenStore>,
    endpoints: Endpoints,
}

impl Accounts {
    /// Settings come from (and are written back to) `config`; a config file
    /// with problems still loads (see `ConfigStore::diagnostics`).
    pub fn load(config: ConfigStore, store: Arc<dyn TokenStore>) -> Accounts {
        Self::with_endpoints(config, store, Endpoints::default())
    }

    pub fn with_endpoints(
        config: ConfigStore,
        store: Arc<dyn TokenStore>,
        endpoints: Endpoints,
    ) -> Accounts {
        Accounts {
            config,
            store,
            endpoints,
        }
    }

    pub fn config(&self) -> &Config {
        self.config.config()
    }

    pub fn config_store(&self) -> &ConfigStore {
        &self.config
    }

    /// Give the config store back (the app keeps one store for the whole
    /// config file and lends it to `Accounts` for a call or two).
    pub fn into_config_store(self) -> ConfigStore {
        self.config
    }

    /// The service endpoints (the app reuses the ChatGPT OAuth settings to
    /// run the browser sign-in off the UI thread).
    pub fn endpoints(&self) -> &Endpoints {
        &self.endpoints
    }

    /// Store Sign-in-with-ChatGPT credentials obtained elsewhere (e.g. from
    /// `ChatGptOAuth::finish_browser_login` on a background thread) and
    /// switch the provider on.
    pub fn store_chatgpt_credentials(&mut self, c: Credentials) -> Result<()> {
        self.store_chatgpt(c)
    }

    /// Re-read the config file (after the user edited it).
    pub fn reload(&mut self) {
        self.config.reload();
    }

    /// `ai-style-prompt`: extra instructions for every generation.
    pub fn style_prompt(&self) -> Option<String> {
        self.config().ai_style_prompt().map(str::to_string)
    }

    fn write(&mut self, changes: &[(&str, Change)]) -> Result<()> {
        self.config
            .set(changes)
            .map_err(|e| AiError::Storage(format!("config: {e}")))
    }

    fn enabled(&self, kind: ProviderKind) -> bool {
        enabled_in(self.config(), kind)
    }

    fn blyg_base_url(&self) -> Result<String> {
        match &self.endpoints.blyg_base_url {
            Some(u) => Ok(u.clone()),
            None => self
                .config()
                .blyg_url()
                .map(str::to_string)
                .ok_or_else(|| AiError::NotConfigured("not connected to a blyg".into())),
        }
    }

    fn secret(&self, account: &str) -> Result<Option<String>> {
        Ok(self.store.get(account)?.filter(|s| !s.is_empty()))
    }

    pub fn status(&self, kind: ProviderKind) -> ProviderStatus {
        let has = |acct: &str| matches!(self.secret(acct), Ok(Some(_)));
        let set = |b: bool| {
            if b {
                ProviderStatus::SignedIn
            } else {
                ProviderStatus::NotSet
            }
        };
        let cli = |name: &str| match self.endpoints.cli.find(name) {
            Some(p) => ProviderStatus::CliFound(p),
            None => ProviderStatus::CliNotFound,
        };
        match kind {
            ProviderKind::AnthropicApi => set(has(ANTHROPIC_KEY_ACCOUNT)),
            ProviderKind::OpenaiApi => set(has(OPENAI_KEY_ACCOUNT)),
            ProviderKind::CloudflareWorkersAi => set(has(cloudflare::KEYCHAIN_ACCOUNT)
                && self.config().cloudflare_account_id().is_some()),
            ProviderKind::ChatgptAccount => match Credentials::load(self.store.as_ref()) {
                Ok(Some(c)) if c.invalid => ProviderStatus::TokenExpired,
                Ok(Some(_)) => ProviderStatus::SignedIn,
                _ => ProviderStatus::NotSet,
            },
            ProviderKind::LocalClaudeCode => cli(claude_code::BINARY),
            ProviderKind::LocalCodex => cli(codex::BINARY),
            ProviderKind::BlygServer => set(self
                .blyg_base_url()
                .ok()
                .is_some_and(|u| matches!(self.secret(&u), Ok(Some(_))))),
        }
    }

    /// One row per provider, for the settings list.
    pub fn rows(&self) -> Vec<AccountRow> {
        let default = self.default_provider();
        ProviderKind::ALL
            .iter()
            .map(|&kind| AccountRow {
                kind,
                enabled: self.enabled(kind),
                status: self.status(kind),
                model: model_in(self.config(), kind),
                is_default: default == Some(kind),
            })
            .collect()
    }

    /// Switch a provider on or off (`ai-enable`).
    pub fn set_enabled(&mut self, kind: ProviderKind, on: bool) -> Result<()> {
        let name = kind.config_name();
        let mut list = self.config().ai_enabled();
        let has = list.iter().any(|e| e == name);
        if has == on {
            return Ok(());
        }
        if on {
            list.push(name.to_string());
        } else {
            list.retain(|e| e != name);
        }
        self.write(&[("ai-enable", Change::List(list))])
    }

    /// Set (or clear) a provider's model: `ai-model` for the `ai-provider`,
    /// otherwise its `ai-provider-model` entry.
    pub fn set_model(&mut self, kind: ProviderKind, model: Option<String>) -> Result<()> {
        let model = model
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty());
        let name = kind.config_name();
        if self.config().ai_provider() == Some(name) {
            let change = match model {
                Some(m) => Change::Set(m),
                None => Change::Remove,
            };
            return self.write(&[("ai-model", change)]);
        }
        let mut list: Vec<String> = self
            .config()
            .ai_provider_models()
            .into_iter()
            .filter(|(p, _)| p != name)
            .map(|(p, m)| format!("{p}={m}"))
            .collect();
        if let Some(m) = model {
            list.push(format!("{name}={m}"));
        }
        let change = if list.is_empty() {
            Change::Remove
        } else {
            Change::List(list)
        };
        self.write(&[("ai-provider-model", change)])
    }

    /// Set (or clear) the default provider (`ai-provider`), optionally with
    /// its model (`ai-model`).
    pub fn set_default(&mut self, kind: Option<ProviderKind>, model: Option<String>) -> Result<()> {
        let mut changes = vec![(
            "ai-provider",
            match kind {
                Some(k) => Change::Set(k.config_name().to_string()),
                None => Change::Remove,
            },
        )];
        if let (Some(_), Some(m)) = (kind, model.filter(|m| !m.trim().is_empty())) {
            changes.push(("ai-model", Change::Set(m.trim().to_string())));
        }
        self.write(&changes)
    }

    /// The configured default if it's enabled and ready, else the first
    /// enabled + ready provider in preference order.
    pub fn default_provider(&self) -> Option<ProviderKind> {
        let usable = |k: ProviderKind| self.enabled(k) && self.status(k).is_ready();
        self.config()
            .ai_provider()
            .and_then(ProviderKind::from_config_name)
            .filter(|&k| usable(k))
            .or_else(|| PREFERENCE.into_iter().find(|&k| usable(k)))
    }

    /// Store credentials and switch the provider on.
    pub fn sign_in(&mut self, kind: ProviderKind, cred: Credential) -> Result<()> {
        match (kind, cred) {
            (ProviderKind::AnthropicApi, Credential::ApiKey(k)) => {
                self.store.set(ANTHROPIC_KEY_ACCOUNT, k.trim())?
            }
            (ProviderKind::OpenaiApi, Credential::ApiKey(k)) => {
                self.store.set(OPENAI_KEY_ACCOUNT, k.trim())?
            }
            (
                ProviderKind::CloudflareWorkersAi,
                Credential::Cloudflare {
                    account_id,
                    api_token,
                },
            ) => {
                self.store
                    .set(cloudflare::KEYCHAIN_ACCOUNT, api_token.trim())?;
                self.write(&[(
                    "cloudflare-account-id",
                    Change::Set(account_id.trim().to_string()),
                )])?;
            }
            (
                ProviderKind::LocalClaudeCode | ProviderKind::LocalCodex | ProviderKind::BlygServer,
                _,
            ) => {}
            (ProviderKind::ChatgptAccount, _) => {
                return Err(AiError::NotConfigured(
                    "use the ChatGPT browser or device-code sign-in".into(),
                ));
            }
            (k, _) => {
                return Err(AiError::NotConfigured(format!(
                    "wrong kind of credential for {}",
                    k.label()
                )));
            }
        }
        self.set_enabled(kind, true)
    }

    /// Forget credentials and switch the provider off. (The blyg owner token
    /// belongs to blyg-core and is left alone.)
    pub fn sign_out(&mut self, kind: ProviderKind) -> Result<()> {
        match kind {
            ProviderKind::AnthropicApi => self.store.delete(ANTHROPIC_KEY_ACCOUNT)?,
            ProviderKind::OpenaiApi => self.store.delete(OPENAI_KEY_ACCOUNT)?,
            ProviderKind::ChatgptAccount => self.store.delete(chatgpt::KEYCHAIN_ACCOUNT)?,
            ProviderKind::CloudflareWorkersAi => {
                self.store.delete(cloudflare::KEYCHAIN_ACCOUNT)?;
                self.write(&[("cloudflare-account-id", Change::Remove)])?;
            }
            ProviderKind::LocalClaudeCode | ProviderKind::LocalCodex | ProviderKind::BlygServer => {
            }
        }
        self.set_enabled(kind, false)
    }

    // ---------- Sign in with ChatGPT (unofficial; see docs/AI.md) ----------

    /// Step 1 of the browser flow: open `login.url` in the browser.
    pub fn start_chatgpt_browser_sign_in(&self) -> Result<BrowserLogin> {
        self.endpoints.chatgpt_oauth.start_browser_login()
    }

    /// Step 2: wait for the localhost callback, exchange, store.
    pub fn finish_chatgpt_browser_sign_in(
        &mut self,
        login: &BrowserLogin,
        cancel: &CancelFlag,
        timeout: Duration,
    ) -> Result<()> {
        let c = self
            .endpoints
            .chatgpt_oauth
            .finish_browser_login(login, cancel, timeout)?;
        self.store_chatgpt(c)
    }

    /// Step 2 (fallback): the user pasted the redirect URL or code.
    pub fn finish_chatgpt_pasted(&mut self, login: &BrowserLogin, input: &str) -> Result<()> {
        let c = self
            .endpoints
            .chatgpt_oauth
            .finish_with_pasted(login, input)?;
        self.store_chatgpt(c)
    }

    /// Device-code flow step 1: show `user_code` and `verification_uri`.
    pub fn start_chatgpt_device_sign_in(&self) -> Result<DeviceCode> {
        self.endpoints.chatgpt_oauth.start_device()
    }

    /// Device-code flow step 2: poll until approved, then store.
    pub fn finish_chatgpt_device_sign_in(
        &mut self,
        code: &DeviceCode,
        cancel: &CancelFlag,
    ) -> Result<()> {
        let c = self.endpoints.chatgpt_oauth.poll_device(code, cancel)?;
        self.store_chatgpt(c)
    }

    fn store_chatgpt(&mut self, c: Credentials) -> Result<()> {
        c.save(self.store.as_ref())?;
        self.set_enabled(ProviderKind::ChatgptAccount, true)
    }

    // ---------- building providers ----------

    /// Build a provider regardless of `enabled` (e.g. for "Test connection").
    pub fn provider(&self, kind: ProviderKind) -> Result<Box<dyn Provider>> {
        let model = model_in(self.config(), kind);
        let need = |acct: &str, what: &str| -> Result<String> {
            self.secret(acct)?
                .ok_or_else(|| AiError::NotConfigured(format!("{what} is not set")))
        };
        let e = &self.endpoints;
        Ok(match kind {
            ProviderKind::AnthropicApi => {
                let mut p =
                    AnthropicApi::new(need(ANTHROPIC_KEY_ACCOUNT, "the Anthropic API key")?)
                        .with_base_url(&e.anthropic);
                if let Some(m) = model {
                    p = p.with_model(m);
                }
                Box::new(p)
            }
            ProviderKind::OpenaiApi => {
                let mut p = OpenAiApi::new(need(OPENAI_KEY_ACCOUNT, "the OpenAI API key")?)
                    .with_base_url(&e.openai);
                if let Some(m) = model {
                    p = p.with_model(m);
                }
                Box::new(p)
            }
            ProviderKind::ChatgptAccount => {
                let mut p = ChatGptAccount::new(self.store.clone())
                    .with_oauth(e.chatgpt_oauth.clone())
                    .with_base_url(&e.chatgpt);
                if let Some(m) = model {
                    p = p.with_model(m);
                }
                Box::new(p)
            }
            ProviderKind::CloudflareWorkersAi => {
                let account = self
                    .config()
                    .cloudflare_account_id()
                    .map(str::to_string)
                    .ok_or_else(|| {
                        AiError::NotConfigured("the Cloudflare account ID is not set".into())
                    })?;
                let mut p = CloudflareWorkersAi::new(
                    account,
                    need(cloudflare::KEYCHAIN_ACCOUNT, "the Cloudflare API token")?,
                )
                .with_base_url(&e.cloudflare);
                if let Some(m) = model {
                    p = p.with_model(m);
                }
                Box::new(p)
            }
            ProviderKind::LocalClaudeCode => Box::new(
                LocalClaudeCode::new()
                    .with_locator(e.cli.clone())
                    .with_model(model),
            ),
            ProviderKind::LocalCodex => Box::new(
                LocalCodex::new()
                    .with_locator(e.cli.clone())
                    .with_model(model),
            ),
            ProviderKind::BlygServer => {
                let base = self.blyg_base_url()?;
                let token = self
                    .secret(&base)?
                    .ok_or_else(|| AiError::NotConfigured("not connected to a blyg".into()))?;
                Box::new(BlygServer::new(&base, token))
            }
        })
    }

    /// The provider for a request: `choice` if given, else the default.
    /// It must be enabled and ready.
    pub fn pick(&self, choice: Option<ProviderKind>) -> Result<Box<dyn Provider>> {
        let kind = match choice {
            Some(k) => k,
            None => self.default_provider().ok_or_else(|| {
                AiError::NotConfigured(
                    "no AI provider is set up; see Settings › AI accounts".into(),
                )
            })?,
        };
        if !self.enabled(kind) {
            return Err(AiError::NotConfigured(format!(
                "{} is switched off",
                kind.label()
            )));
        }
        match self.status(kind) {
            ProviderStatus::TokenExpired => return Err(AiError::TokenExpired(kind.label().into())),
            ProviderStatus::CliNotFound => return Err(AiError::CliNotFound(kind.label().into())),
            ProviderStatus::NotSet => {
                return Err(AiError::NotConfigured(format!(
                    "{} is not signed in",
                    kind.label()
                )));
            }
            _ => {}
        }
        self.provider(kind)
    }
}
