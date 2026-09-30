//! AI in the app: the provider registry built from the config file and the
//! Keychain (`blyg_ai::Accounts`), the TK / helper logic, and the words the
//! UI shows. Nothing here draws pixels; the main window's AI UI lives in
//! `ai/view.rs` (a child of `app`, so it can reach the window's state).
//!
//! AI is off until the user enables a provider (`ai-enable` in the config
//! file, or Settings › AI) or signs in to one. See docs/AI.md.

pub mod generate;
pub mod palette;
pub mod settings;

use std::sync::Arc;
use std::time::Duration;

use blyg_ai::{Accounts, AiError, Endpoints, Provider, ProviderKind};
use blyg_core::ConfigStore;
use gpui_kit::{App, Global};

gpui_kit::actions!(blygger_ai, [AiGenerate, AiShorten]);

/// How long a generation may run before the app gives up on it.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(240);

/// App-wide AI state (a GPUI global): test doubles and limits. The
/// providers themselves are built per request from the config + Keychain.
#[derive(Default)]
pub struct AiGlobal {
    /// Tests: use this provider instead of anything configured.
    pub test_provider: Option<Arc<dyn Provider>>,
    /// `None` = [`DEFAULT_TIMEOUT`].
    pub timeout: Option<Duration>,
    /// `None` = the real services (tests point these at nothing).
    pub endpoints: Option<Endpoints>,
}

impl Global for AiGlobal {}

pub fn init(cx: &mut App) {
    if !cx.has_global::<AiGlobal>() {
        cx.set_global(AiGlobal::default());
    }
}

pub fn timeout(cx: &App) -> Duration {
    cx.try_global::<AiGlobal>()
        .and_then(|g| g.timeout)
        .unwrap_or(DEFAULT_TIMEOUT)
}

/// Lend the app's one config store (and the Keychain) to `Accounts` for
/// `f`, then take it back. Accounts reads and writes the AI keys of the
/// same file the rest of the app uses, through the same edit code.
pub fn with_accounts<R>(cx: &mut App, f: impl FnOnce(&mut Accounts) -> R) -> R {
    let endpoints = cx
        .try_global::<AiGlobal>()
        .and_then(|g| g.endpoints.clone())
        .unwrap_or_default();
    let (store, tokens) = {
        let g = cx.global_mut::<crate::settings::AppConfig>();
        (
            std::mem::replace(&mut g.store, ConfigStore::in_memory("")),
            g.tokens.clone(),
        )
    };
    let mut accounts = Accounts::with_endpoints(store, tokens, endpoints);
    let r = f(&mut accounts);
    cx.global_mut::<crate::settings::AppConfig>().store = accounts.into_config_store();
    r
}

/// A provider ready to run, and what to call it in the status bar.
#[derive(Clone)]
pub struct Picked {
    pub provider: Arc<dyn Provider>,
    pub kind: ProviderKind,
}

/// The provider for a generation: the default one (`ai-provider`, else the
/// first enabled and ready one). An error says what's missing.
/// On failure, also says which provider it was trying (for the toast).
pub fn pick(cx: &mut App) -> Result<Picked, (AiError, Option<ProviderKind>)> {
    if let Some(p) = cx
        .try_global::<AiGlobal>()
        .and_then(|g| g.test_provider.clone())
    {
        return Ok(Picked {
            kind: p.kind(),
            provider: p,
        });
    }
    with_accounts(cx, |a| {
        // Nothing ready: name the first enabled provider's own problem (not
        // installed, not signed in) rather than a generic "none set up".
        let kind = a.default_provider().or_else(|| {
            a.config()
                .ai_enabled()
                .iter()
                .find_map(|n| ProviderKind::from_config_name(n))
        });
        let provider = a.pick(kind).map_err(|e| (e, kind))?;
        Ok(Picked {
            kind: provider.kind(),
            provider: Arc::from(provider),
        })
    })
}

/// `ai-style-prompt`.
pub fn style_prompt(cx: &App) -> Option<String> {
    crate::settings::get(cx)
        .store
        .config()
        .ai_style_prompt()
        .map(str::to_string)
}

/// Why a generation stopped, beyond the provider's own errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    Ai(AiError),
    /// The app's own limit ([`timeout`]) ran out.
    TimedOut(Duration),
}

/// The toast for a failed generation: a headline and what to do about it.
/// Never includes secrets (provider messages are passed through [`redact`]).
pub fn failure_toast(f: &Failure, provider: Option<ProviderKind>) -> (String, String) {
    let who = provider.map(|k| k.label()).unwrap_or("The AI provider");
    let fix_in_settings =
        "Open Settings › AI (⌘,) to set one up, or add ai-enable to the config file";
    match f {
        Failure::TimedOut(d) => (
            format!("{who} took longer than {} s; nothing was written", d.as_secs()),
            "Try again, or pick a faster model in Settings › AI".into(),
        ),
        Failure::Ai(e) => match e {
            AiError::Cancelled => ("Generation cancelled".into(), "Nothing was written".into()),
            AiError::NotConfigured(m) => (
                format!("AI isn't ready: {}", redact(m)),
                fix_in_settings.into(),
            ),
            AiError::CliNotFound(name) => (
                format!("{name} isn't installed"),
                match provider {
                    Some(ProviderKind::LocalCodex) => {
                        "Install the codex CLI (and run `codex login`), or pick another provider in Settings › AI".into()
                    }
                    Some(ProviderKind::LocalClaudeCode) => {
                        "Install Claude Code (and sign in with `claude`), or pick another provider in Settings › AI".into()
                    }
                    _ => "Install it, or pick another provider in Settings › AI".into(),
                },
            ),
            AiError::CliFailed {
                name,
                code,
                message,
            } => (
                match code {
                    Some(c) => format!("{name} stopped with exit code {c}"),
                    None => format!("{name} stopped"),
                },
                {
                    let m = short(&redact(message), 140);
                    let hint = "Run it once in a terminal to check it's signed in";
                    if m.is_empty() {
                        hint.to_string()
                    } else {
                        format!("{m} · {hint}")
                    }
                },
            ),
            AiError::Unauthorized { provider: p } => (
                format!("{p} didn't accept the credentials"),
                "Check the key, or sign in again, in Settings › AI".into(),
            ),
            AiError::TokenExpired(p) => (
                format!("Your {p} sign-in has expired"),
                "Sign in again in Settings › AI".into(),
            ),
            AiError::Refused(m) => (
                short(&redact(m), 160),
                "Reword the instruction and try again".into(),
            ),
            AiError::Network(m) => (
                format!("Couldn't reach {who}"),
                format!("{} · check your connection", short(&redact(m), 120)),
            ),
            AiError::Provider(m) => (
                format!("{who} failed"),
                short(&redact(m), 160),
            ),
            AiError::Storage(m) => (
                "Couldn't read the AI settings".into(),
                short(&redact(m), 160),
            ),
            AiError::Prompt(m) => (
                short(&redact(m), 160),
                "Fix the [TK]…[/TK] markup and try again".into(),
            ),
        },
    }
}

fn short(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// Blank out anything that looks like a credential (API keys, bearer
/// tokens, JWTs) before a message reaches the screen.
pub fn redact(s: &str) -> String {
    s.split_inclusive(char::is_whitespace)
        .map(|w| {
            let word = w.trim_end();
            let tail = &w[word.len()..];
            let core = word.trim_matches(|c: char| "\"'`()[]{},;:".contains(c));
            let secretish = core.starts_with("sk-")
                || core.starts_with("sk_")
                || core.starts_with("eyJ")
                || (core.len() >= 32
                    && core
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_.=+/".contains(c))
                    && core.chars().any(|c| c.is_ascii_digit())
                    && core.chars().any(|c| c.is_ascii_alphabetic()));
            if secretish {
                format!("{}{tail}", word.replace(core, "[redacted]"))
            } else {
                w.to_string()
            }
        })
        .collect()
}

#[cfg(test)]
pub mod tests_support {
    //! A scripted provider for tests: no network, no CLI.

    use std::sync::Mutex;
    use std::time::Duration;

    use blyg_ai::{AiError, GenRequest, GenResult, Provider, ProviderKind, Result};

    pub struct FakeProvider {
        pub kind: ProviderKind,
        pub model: String,
        /// Replies in order; the last one repeats.
        pub replies: Mutex<Vec<Result<String>>>,
        /// Every request seen (system, user).
        pub seen: Mutex<Vec<(String, String)>>,
        /// Block until cancelled (to test esc).
        pub wait_for_cancel: bool,
        pub delay: Duration,
    }

    impl FakeProvider {
        pub fn replying(texts: &[&str]) -> Self {
            FakeProvider {
                kind: ProviderKind::LocalCodex,
                model: "fake-model-1".into(),
                replies: Mutex::new(texts.iter().map(|t| Ok(t.to_string())).collect()),
                seen: Mutex::new(vec![]),
                wait_for_cancel: false,
                delay: Duration::ZERO,
            }
        }
        pub fn failing(e: AiError) -> Self {
            let p = Self::replying(&[]);
            *p.replies.lock().unwrap() = vec![Err(e)];
            p
        }
        pub fn hanging() -> Self {
            FakeProvider {
                wait_for_cancel: true,
                ..Self::replying(&["never"])
            }
        }
    }

    impl Provider for FakeProvider {
        fn kind(&self) -> ProviderKind {
            self.kind
        }
        fn generate(&self, req: GenRequest, on_delta: &mut dyn FnMut(&str)) -> Result<GenResult> {
            self.seen
                .lock()
                .unwrap()
                .push((req.system.clone(), req.user.clone()));
            if self.wait_for_cancel {
                for _ in 0..2000 {
                    if req.cancel.is_cancelled() {
                        return Err(AiError::Cancelled);
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                return Err(AiError::Cancelled);
            }
            if !self.delay.is_zero() {
                std::thread::sleep(self.delay);
            }
            req.cancel.check()?;
            let mut r = self.replies.lock().unwrap();
            let next = if r.len() > 1 {
                r.remove(0)
            } else {
                r.first().cloned().unwrap_or(Ok(String::new()))
            };
            let text = next?;
            on_delta(&text);
            Ok(GenResult {
                text,
                model: self.model.clone(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_things_that_look_like_keys() {
        let s = redact("bad key sk-ant-api03-abcdefghijklmnop for \"eyJhbGciOi.x.y\" ok");
        assert!(!s.contains("sk-ant"), "{s}");
        assert!(!s.contains("eyJ"), "{s}");
        assert!(s.contains("bad key") && s.ends_with(" ok"), "{s}");
        let t = "token 0123456789abcdefghijklmnopqrstuvwxyzABCD rejected";
        assert!(!redact(t).contains("0123456789abcdef"));
        assert_eq!(redact("plain words stay"), "plain words stay");
    }

    #[test]
    fn toasts_say_what_to_do() {
        let (h, s) = failure_toast(
            &Failure::Ai(AiError::CliNotFound("Codex (local)".into())),
            Some(ProviderKind::LocalCodex),
        );
        assert!(h.contains("isn't installed"));
        assert!(s.contains("codex"));
        let (h, s) = failure_toast(
            &Failure::Ai(AiError::NotConfigured("no AI provider is set up".into())),
            None,
        );
        assert!(h.contains("isn't ready") && s.contains("Settings › AI"));
        let (h, _) = failure_toast(&Failure::TimedOut(Duration::from_secs(9)), None);
        assert!(h.contains("9 s"));
        let (h, s) = failure_toast(
            &Failure::Ai(AiError::CliFailed {
                name: "codex".into(),
                code: Some(2),
                message: "auth sk-live-0123456789abcdefghijklmnop failed".into(),
            }),
            Some(ProviderKind::LocalCodex),
        );
        assert!(h.contains("exit code 2"));
        assert!(!s.contains("sk-live"), "{s}");
    }
}
