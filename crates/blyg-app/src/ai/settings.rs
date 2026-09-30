//! Settings › AI, UI-free: provider rows and their status words, and the
//! writes (config keys through `Accounts`, which uses the config edit
//! module; secrets straight to the Keychain).
//!
//! Secrets are never read back for display, never logged, and never part
//! of any message: a stored key shows as "saved" with a Remove button.
//! There is no claude.ai login, ever (Anthropic's terms; docs/AI.md).

use blyg_ai::{AccountRow, Credential, ProviderKind, ProviderStatus};
use gpui_kit::App;

use super::with_accounts;

/// How a status reads, and whether it's good news.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Good,
    Neutral,
    Bad,
}

/// The providers in the order Settings lists them.
pub const ORDER: [ProviderKind; 7] = [
    ProviderKind::LocalClaudeCode,
    ProviderKind::LocalCodex,
    ProviderKind::ChatgptAccount,
    ProviderKind::AnthropicApi,
    ProviderKind::OpenaiApi,
    ProviderKind::CloudflareWorkersAi,
    ProviderKind::BlygServer,
];

/// The status word for a row: enabled / signed in / not installed / ….
pub fn status_label(row: &AccountRow) -> (&'static str, Tone) {
    match (&row.status, row.enabled) {
        (ProviderStatus::CliNotFound, _) => ("not installed", Tone::Bad),
        (ProviderStatus::TokenExpired, _) => ("sign in again", Tone::Bad),
        (ProviderStatus::CliFound(_), true) => ("enabled", Tone::Good),
        (ProviderStatus::CliFound(_), false) => ("installed · off", Tone::Neutral),
        (ProviderStatus::SignedIn, true) => ("signed in", Tone::Good),
        (ProviderStatus::SignedIn, false) => ("signed in · off", Tone::Neutral),
        (ProviderStatus::NotSet, true) => ("enabled · not signed in", Tone::Bad),
        (ProviderStatus::NotSet, false) => ("not set", Tone::Neutral),
    }
}

/// One line on how to connect each provider.
pub fn how(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::LocalClaudeCode => {
            "Runs your installed `claude` CLI, signed in to your own Claude plan"
        }
        ProviderKind::LocalCodex => {
            "Runs your installed `codex` CLI, signed in to your own account"
        }
        ProviderKind::ChatgptAccount => "Sign in with ChatGPT in the browser (unofficial)",
        ProviderKind::AnthropicApi => {
            crate::keymap::hint("An Anthropic API key, kept in the Keychain")
        }
        ProviderKind::OpenaiApi => crate::keymap::hint("An OpenAI API key, kept in the Keychain"),
        ProviderKind::CloudflareWorkersAi => {
            crate::keymap::hint("Account ID + API token (the token goes in the Keychain)")
        }
        ProviderKind::BlygServer => "Your blyg's own /generate endpoint (the Worker needs a key)",
    }
}

/// Providers that switch on with no credential (the CLIs, the blyg).
pub fn toggles_freely(kind: ProviderKind) -> bool {
    matches!(
        kind,
        ProviderKind::LocalClaudeCode | ProviderKind::LocalCodex | ProviderKind::BlygServer
    )
}

pub fn rows(cx: &mut App) -> Vec<AccountRow> {
    let mut rows = with_accounts(cx, |a| a.rows());
    rows.sort_by_key(|r| ORDER.iter().position(|k| *k == r.kind));
    rows
}

/// Words for a failed settings write, without echoing anything secret.
fn err(e: blyg_ai::AiError) -> String {
    super::redact(&e.to_string())
}

pub fn set_enabled(kind: ProviderKind, on: bool, cx: &mut App) -> Result<(), String> {
    with_accounts(cx, |a| a.set_enabled(kind, on)).map_err(err)
}

pub fn set_default(kind: ProviderKind, cx: &mut App) -> Result<(), String> {
    with_accounts(cx, |a| a.set_default(Some(kind), None)).map_err(err)
}

pub fn set_model(kind: ProviderKind, model: &str, cx: &mut App) -> Result<(), String> {
    let m = model.trim();
    with_accounts(cx, |a| {
        a.set_model(kind, (!m.is_empty()).then(|| m.to_string()))
    })
    .map_err(err)
}

/// Store an API key (Anthropic / OpenAI) and switch the provider on. The
/// key goes to the Keychain only; the caller clears its input field.
pub fn save_api_key(kind: ProviderKind, key: &str, cx: &mut App) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("Paste the key first".into());
    }
    if key.chars().any(char::is_whitespace) {
        return Err("That doesn't look like a key (it has spaces in it)".into());
    }
    with_accounts(cx, |a| a.sign_in(kind, Credential::ApiKey(key.to_string()))).map_err(err)
}

pub fn save_cloudflare(account_id: &str, token: &str, cx: &mut App) -> Result<(), String> {
    let (account_id, token) = (account_id.trim(), token.trim());
    if account_id.is_empty() || token.is_empty() {
        return Err("Enter both the account ID and the API token".into());
    }
    with_accounts(cx, |a| {
        a.sign_in(
            ProviderKind::CloudflareWorkersAi,
            Credential::Cloudflare {
                account_id: account_id.to_string(),
                api_token: token.to_string(),
            },
        )
    })
    .map_err(err)
}

/// Forget the stored credential and switch the provider off.
pub fn remove(kind: ProviderKind, cx: &mut App) -> Result<(), String> {
    with_accounts(cx, |a| a.sign_out(kind)).map_err(err)
}

/// The data dir for `state.json` (the ChatGPT notice), or none in tests.
pub fn notice_shown(data_dir: Option<&std::path::Path>) -> bool {
    data_dir.is_some_and(|d| blyg_core::state::AppState::load(d).chatgpt_notice_shown)
}

pub fn mark_notice_shown(data_dir: Option<&std::path::Path>) {
    if let Some(d) = data_dir {
        let _ = blyg_core::state::AppState::update(d, |s| s.chatgpt_notice_shown = true);
    }
}

pub const CHATGPT_NOTICE: &str = "Sign in with ChatGPT is unofficial. OpenAI offers no \
    sanctioned way for other apps to use a ChatGPT plan; this borrows the Codex CLI's sign-in, \
    and OpenAI may change or block it at any time.";

#[cfg(test)]
mod tests {
    use super::*;

    fn row(status: ProviderStatus, enabled: bool) -> AccountRow {
        AccountRow {
            kind: ProviderKind::LocalCodex,
            enabled,
            status,
            model: None,
            is_default: false,
        }
    }

    #[test]
    fn status_words() {
        assert_eq!(
            status_label(&row(ProviderStatus::CliNotFound, true)).0,
            "not installed"
        );
        assert_eq!(
            status_label(&row(ProviderStatus::CliFound("/x".into()), true)).0,
            "enabled"
        );
        assert_eq!(
            status_label(&row(ProviderStatus::SignedIn, true)).0,
            "signed in"
        );
        assert_eq!(
            status_label(&row(ProviderStatus::NotSet, false)).0,
            "not set"
        );
    }

    #[test]
    fn nothing_offers_a_claude_ai_login() {
        for k in ORDER {
            assert!(!how(k).to_lowercase().contains("claude.ai"), "{k:?}");
        }
    }
}
