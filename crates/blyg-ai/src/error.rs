//! One error type for every provider, so the UI can show a clean message and
//! decide whether "sign in again" is the right call to action.

use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AiError {
    /// The shared cancel flag was set while generating.
    #[error("cancelled")]
    Cancelled,
    /// Provider isn't set up (no key, not signed in, disabled).
    #[error("{0}")]
    NotConfigured(String),
    /// The provider rejected the credentials (HTTP 401/403).
    #[error("not authorised by {provider}; sign in again or check the key")]
    Unauthorized { provider: String },
    /// Stored OAuth tokens can't be refreshed any more; the user must sign in again.
    #[error("your {0} sign-in has expired; sign in again")]
    TokenExpired(String),
    /// The model declined (Anthropic `stop_reason: "refusal"`, OpenAI refusal output).
    #[error("{0}")]
    Refused(String),
    /// Transport failure (no network, DNS, TLS, timeouts).
    #[error("network error: {0}")]
    Network(String),
    /// Any other provider-side failure, surfaced verbatim.
    #[error("{0}")]
    Provider(String),
    /// A local CLI bridge couldn't find its binary.
    #[error("{0} not found; install it or check your PATH")]
    CliNotFound(String),
    /// A local CLI ran but failed.
    #[error("{name} failed (exit {code:?}): {message}")]
    CliFailed {
        name: String,
        code: Option<i32>,
        message: String,
    },
    /// Keychain / config file problems.
    #[error("storage: {0}")]
    Storage(String),
    /// A prompt couldn't be built (bad TK scope index, malformed scopes, …).
    #[error("{0}")]
    Prompt(String),
}

pub type Result<T> = std::result::Result<T, AiError>;

impl From<blyg_core::CoreError> for AiError {
    fn from(e: blyg_core::CoreError) -> Self {
        AiError::Storage(e.to_string())
    }
}
