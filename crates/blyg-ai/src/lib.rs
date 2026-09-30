//! blyg-ai: AI providers, sign-in, and the TK / helper prompts.
//!
//! - [`provider`]: the `Provider` trait (blocking, streaming, cancellable).
//! - Providers: [`anthropic::AnthropicApi`], [`openai::OpenAiApi`],
//!   [`chatgpt::ChatGptAccount`] (unofficial Sign in with ChatGPT),
//!   [`claude_code::LocalClaudeCode`], [`codex::LocalCodex`],
//!   [`cloudflare::CloudflareWorkersAi`], [`blyg_server::BlygServer`].
//! - [`prompts`]: TK prompts with server parity, plus the v1 helpers.
//! - [`accounts::Accounts`]: enabled providers, defaults, sign-in state.
//!
//! There is deliberately no claude.ai (Pro/Max) OAuth login: Anthropic's
//! terms prohibit it for third-party apps. Use `LocalClaudeCode` instead.
//! See docs/AI.md.

pub mod accounts;
pub mod anthropic;
pub mod blyg_server;
pub mod chatgpt;
pub mod claude_code;
pub mod cli;
pub mod cloudflare;
pub mod codex;
pub mod error;
mod http;
pub mod openai;
pub mod prompts;
pub mod provider;

pub use accounts::{AccountRow, Accounts, Credential, Endpoints, ProviderStatus};
pub use error::{AiError, Result};
pub use provider::{
    CancelFlag, GenRequest, GenResult, ModelInfo, Provider, ProviderKind, ServerScope,
};

/// Open a URL in the default browser (macOS `open`; on Windows the shell's
/// URL handler, which takes the URL as one argument, `&` and all).
pub fn open_in_browser(url: &str) -> Result<()> {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = std::process::Command::new("rundll32.exe");
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    #[cfg(not(windows))]
    let mut cmd = std::process::Command::new("/usr/bin/open");
    cmd.arg(url)
        .status()
        .map_err(|e| AiError::Provider(format!("couldn't open the browser: {e}")))
        .and_then(|s| {
            if s.success() {
                Ok(())
            } else {
                Err(AiError::Provider("couldn't open the browser".into()))
            }
        })
}
