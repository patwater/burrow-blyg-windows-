//! The provider abstraction. Every provider is blocking (the app runs it on a
//! background thread), streams text through `on_delta`, and stops early when
//! the shared cancel flag is set.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::error::{AiError, Result};

/// Shared cancellation flag. Clone it into the request, keep one for the UI's
/// "stop" button, and call `cancel()`.
#[derive(Debug, Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
    /// `Err(Cancelled)` if the flag is set.
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(AiError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// `POST /api/items/:id/generate {scope}` coordinates, for the blyg-server
/// provider (which builds its own prompt server-side).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerScope {
    /// The item's server id.
    pub item_id: String,
    /// 0-based TK scope index in the *server's* working copy.
    pub scope: usize,
}

/// One generation request: a system prompt plus one user message, which is
/// the shape the blyg server's TK prompt uses (see `prompts.rs`).
#[derive(Debug, Clone, Default)]
pub struct GenRequest {
    pub system: String,
    pub user: String,
    /// `None` = the provider's configured/default model.
    pub model: Option<String>,
    /// Output cap. `None` = the provider's default.
    pub max_tokens: Option<u32>,
    pub cancel: CancelFlag,
    /// Only used by `BlygServer`; other providers ignore it.
    pub server_scope: Option<ServerScope>,
}

impl GenRequest {
    pub fn new(system: impl Into<String>, user: impl Into<String>) -> Self {
        GenRequest {
            system: system.into(),
            user: user.into(),
            ..Default::default()
        }
    }
    pub fn with_model(mut self, model: Option<String>) -> Self {
        self.model = model;
        self
    }
    pub fn with_cancel(mut self, cancel: CancelFlag) -> Self {
        self.cancel = cancel;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenResult {
    pub text: String,
    /// The model that actually answered (for provenance).
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: Option<String>,
}

/// Which provider. See `config_name` for the names used in the config file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    ChatgptAccount,
    OpenaiApi,
    AnthropicApi,
    LocalClaudeCode,
    LocalCodex,
    BlygServer,
    CloudflareWorkersAi,
}

impl ProviderKind {
    pub const ALL: [ProviderKind; 7] = [
        ProviderKind::ChatgptAccount,
        ProviderKind::OpenaiApi,
        ProviderKind::AnthropicApi,
        ProviderKind::LocalClaudeCode,
        ProviderKind::LocalCodex,
        ProviderKind::BlygServer,
        ProviderKind::CloudflareWorkersAi,
    ];

    /// The name used in the config file (`ai-provider`, `ai-enable`, …).
    pub fn config_name(self) -> &'static str {
        match self {
            ProviderKind::ChatgptAccount => "chatgpt",
            ProviderKind::OpenaiApi => "openai",
            ProviderKind::AnthropicApi => "anthropic",
            ProviderKind::LocalClaudeCode => "claude-code",
            ProviderKind::LocalCodex => "codex",
            ProviderKind::BlygServer => "server",
            ProviderKind::CloudflareWorkersAi => "cloudflare",
        }
    }

    pub fn from_config_name(name: &str) -> Option<ProviderKind> {
        ProviderKind::ALL
            .into_iter()
            .find(|k| k.config_name() == name)
    }

    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::ChatgptAccount => "ChatGPT account",
            ProviderKind::OpenaiApi => "OpenAI API key",
            ProviderKind::AnthropicApi => "Anthropic API key",
            ProviderKind::LocalClaudeCode => "Claude Code (local)",
            ProviderKind::LocalCodex => "Codex (local)",
            ProviderKind::BlygServer => "Blyg server",
            ProviderKind::CloudflareWorkersAi => "Cloudflare Workers AI",
        }
    }

    /// The current flagship for API providers; `None` means "whatever the
    /// tool/server is configured to use".
    pub fn default_model(self) -> Option<&'static str> {
        match self {
            ProviderKind::AnthropicApi => Some(crate::anthropic::DEFAULT_MODEL),
            ProviderKind::OpenaiApi => Some(crate::openai::DEFAULT_MODEL),
            ProviderKind::ChatgptAccount => Some(crate::chatgpt::DEFAULT_MODEL),
            ProviderKind::CloudflareWorkersAi => Some(crate::cloudflare::DEFAULT_MODEL),
            ProviderKind::LocalClaudeCode | ProviderKind::LocalCodex | ProviderKind::BlygServer => {
                None
            }
        }
    }
}

pub trait Provider: Send + Sync {
    fn kind(&self) -> ProviderKind;

    /// Generate, streaming text deltas through `on_delta`. Blocking.
    fn generate(&self, req: GenRequest, on_delta: &mut dyn FnMut(&str)) -> Result<GenResult>;

    /// Models this provider can use, where it can list them. `Ok(vec![])`
    /// when listing isn't supported.
    fn list_models(&self) -> Result<Vec<ModelInfo>> {
        Ok(vec![])
    }
}
