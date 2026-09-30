//! Accounts: config-file persistence, status per provider, sign in/out, picking
//! a provider. Uses MemoryTokenStore, mock servers and fake CLIs only.

mod common;

use std::sync::Arc;

use blyg_ai::accounts::ANTHROPIC_KEY_ACCOUNT;
use blyg_ai::chatgpt::Credentials;
use blyg_ai::cli::CliLocator;
use blyg_ai::*;
use blyg_core::config::{ConfigFiles, ConfigStore, MemoryTokenStore, TokenStore};
use common::*;
use serde_json::json;

struct Env {
    dir: tempfile::TempDir,
    bin: tempfile::TempDir,
    store: Arc<MemoryTokenStore>,
}

impl Env {
    fn new() -> Env {
        Env {
            dir: tempfile::tempdir().unwrap(),
            bin: tempfile::tempdir().unwrap(),
            store: Arc::new(MemoryTokenStore::default()),
        }
    }
    fn endpoints(&self) -> Endpoints {
        Endpoints {
            cli: CliLocator {
                // join_paths: `:` on Unix, `;` on Windows.
                path_var: Some(
                    std::env::join_paths([
                        self.bin.path(),
                        std::path::Path::new("/bin"),
                        std::path::Path::new("/usr/bin"),
                    ])
                    .unwrap(),
                ),
                home: None,
                skip_common_dirs: true,
            },
            blyg_base_url: Some("https://blyg.test".into()),
            ..Default::default()
        }
    }
    fn accounts(&self) -> Accounts {
        self.accounts_with(self.endpoints())
    }
    fn config_path(&self) -> std::path::PathBuf {
        self.dir.path().join("config")
    }
    fn config_store(&self) -> ConfigStore {
        ConfigStore::open(ConfigFiles::single(self.config_path()))
    }
    fn accounts_with(&self, e: Endpoints) -> Accounts {
        Accounts::with_endpoints(self.config_store(), self.store.clone(), e)
    }
}

fn row(a: &Accounts, k: ProviderKind) -> AccountRow {
    a.rows().into_iter().find(|r| r.kind == k).unwrap()
}

#[test]
fn fresh_statuses_and_defaults() {
    let env = Env::new();
    let a = env.accounts();
    assert_eq!(a.status(ProviderKind::AnthropicApi), ProviderStatus::NotSet);
    assert_eq!(a.status(ProviderKind::OpenaiApi), ProviderStatus::NotSet);
    assert_eq!(
        a.status(ProviderKind::ChatgptAccount),
        ProviderStatus::NotSet
    );
    assert_eq!(
        a.status(ProviderKind::CloudflareWorkersAi),
        ProviderStatus::NotSet
    );
    assert_eq!(a.status(ProviderKind::BlygServer), ProviderStatus::NotSet);
    assert_eq!(
        a.status(ProviderKind::LocalClaudeCode),
        ProviderStatus::CliNotFound
    );
    assert_eq!(
        a.status(ProviderKind::LocalCodex),
        ProviderStatus::CliNotFound
    );
    // Opt-in: nothing is on until the user enables it, not even the CLIs.
    assert!(!row(&a, ProviderKind::LocalClaudeCode).enabled);
    assert!(!row(&a, ProviderKind::LocalCodex).enabled);
    assert!(!row(&a, ProviderKind::BlygServer).enabled);
    assert!(!row(&a, ProviderKind::AnthropicApi).enabled);
    assert_eq!(
        row(&a, ProviderKind::AnthropicApi).model.as_deref(),
        Some("claude-opus-5")
    );
    assert_eq!(
        row(&a, ProviderKind::OpenaiApi).model.as_deref(),
        Some("gpt-6-astra")
    );
    assert_eq!(
        row(&a, ProviderKind::CloudflareWorkersAi).model.as_deref(),
        Some("@cf/google/gemma-4-26b-a4b-it")
    );
    assert_eq!(a.default_provider(), None);
    assert!(matches!(a.pick(None), Err(AiError::NotConfigured(_))));
    assert_eq!(a.rows().len(), 7);
}

#[test]
fn cli_found_becomes_the_default_once_enabled() {
    let env = Env::new();
    script(env.bin.path(), "claude", "true");
    let mut a = env.accounts();
    assert!(
        matches!(a.status(ProviderKind::LocalClaudeCode), ProviderStatus::CliFound(p) if p.ends_with("claude"))
    );
    // An installed CLI is never used until the user switches it on.
    assert_eq!(a.default_provider(), None);
    assert!(a.pick(None).is_err());
    a.set_enabled(ProviderKind::LocalClaudeCode, true).unwrap();
    assert_eq!(a.default_provider(), Some(ProviderKind::LocalClaudeCode));
    assert_eq!(a.pick(None).unwrap().kind(), ProviderKind::LocalClaudeCode);
    assert!(row(&a, ProviderKind::LocalClaudeCode).is_default);
}

#[test]
fn sign_in_persist_and_sign_out() {
    let env = Env::new();
    let mut a = env.accounts();
    a.sign_in(
        ProviderKind::AnthropicApi,
        Credential::ApiKey("  sk-ant-1 \n".into()),
    )
    .unwrap();
    a.sign_in(
        ProviderKind::CloudflareWorkersAi,
        Credential::Cloudflare {
            account_id: "acc123".into(),
            api_token: "cf-tok".into(),
        },
    )
    .unwrap();
    a.set_default(Some(ProviderKind::CloudflareWorkersAi), None)
        .unwrap();
    a.set_model(ProviderKind::AnthropicApi, Some("claude-sonnet-5".into()))
        .unwrap();

    assert_eq!(
        env.store.get(ANTHROPIC_KEY_ACCOUNT).unwrap().as_deref(),
        Some("sk-ant-1")
    );
    // No secrets in the config file; just the settings.
    let text = std::fs::read_to_string(env.config_path()).unwrap();
    assert!(
        !text.contains("sk-ant") && !text.contains("cf-tok"),
        "{text}"
    );
    assert!(
        text.contains("\ncloudflare-account-id = acc123\n"),
        "{text}"
    );
    assert!(text.contains("\nai-provider = cloudflare\n"), "{text}");
    assert!(
        text.contains("\nai-provider-model = anthropic=claude-sonnet-5\n"),
        "{text}"
    );
    assert!(
        text.contains("\nai-enable = anthropic\nai-enable = cloudflare\n"),
        "{text}"
    );

    // Reload from disk.
    let mut a = env.accounts();
    assert_eq!(
        a.status(ProviderKind::AnthropicApi),
        ProviderStatus::SignedIn
    );
    assert_eq!(
        a.status(ProviderKind::CloudflareWorkersAi),
        ProviderStatus::SignedIn
    );
    assert!(row(&a, ProviderKind::AnthropicApi).enabled);
    assert_eq!(
        row(&a, ProviderKind::AnthropicApi).model.as_deref(),
        Some("claude-sonnet-5")
    );
    assert_eq!(
        a.default_provider(),
        Some(ProviderKind::CloudflareWorkersAi)
    );
    assert_eq!(
        a.pick(None).unwrap().kind(),
        ProviderKind::CloudflareWorkersAi
    );
    assert_eq!(
        a.pick(Some(ProviderKind::AnthropicApi)).unwrap().kind(),
        ProviderKind::AnthropicApi
    );
    assert!(matches!(
        a.pick(Some(ProviderKind::OpenaiApi)),
        Err(AiError::NotConfigured(_))
    ));

    a.sign_out(ProviderKind::CloudflareWorkersAi).unwrap();
    assert_eq!(
        a.status(ProviderKind::CloudflareWorkersAi),
        ProviderStatus::NotSet
    );
    assert!(!row(&a, ProviderKind::CloudflareWorkersAi).enabled);
    // Default falls back to the next ready provider.
    assert_eq!(a.default_provider(), Some(ProviderKind::AnthropicApi));

    a.set_enabled(ProviderKind::AnthropicApi, false).unwrap();
    assert!(
        matches!(a.pick(Some(ProviderKind::AnthropicApi)), Err(AiError::NotConfigured(m)) if m.contains("switched off"))
    );
    assert!(
        a.sign_in(ProviderKind::ChatgptAccount, Credential::ApiKey("x".into()))
            .is_err()
    );
}

#[test]
fn bad_config_values_are_diagnostics_and_comments_survive_writes() {
    let env = Env::new();
    std::fs::write(
        env.config_path(),
        "# my AI settings\nai-provider = bogus\nai-enable = anthropic\n# end\n",
    )
    .unwrap();
    let mut a = env.accounts();
    let d = a.config_store().diagnostics();
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].line, 2);
    assert_eq!(
        a.config().ai_provider(),
        None,
        "a bad value keeps the default"
    );
    assert!(row(&a, ProviderKind::AnthropicApi).enabled);
    assert!(!row(&a, ProviderKind::LocalClaudeCode).enabled);

    a.set_enabled(ProviderKind::OpenaiApi, true).unwrap();
    a.set_default(Some(ProviderKind::OpenaiApi), Some("gpt-x".into()))
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(env.config_path()).unwrap(),
        "# my AI settings\nai-provider = openai\nai-enable = anthropic\nai-enable = openai\n# end\n\nai-model = gpt-x\n"
    );
    assert_eq!(
        row(&a, ProviderKind::OpenaiApi).model.as_deref(),
        Some("gpt-x")
    );
    // Changing the default provider's model edits ai-model in place.
    a.set_model(ProviderKind::OpenaiApi, Some("gpt-y".into()))
        .unwrap();
    assert!(
        std::fs::read_to_string(env.config_path())
            .unwrap()
            .ends_with("\nai-model = gpt-y\n")
    );
    // A style prompt from the file.
    std::fs::write(env.config_path(), "ai-style-prompt = \"Plain words.\"\n").unwrap();
    a.reload();
    assert_eq!(a.style_prompt().as_deref(), Some("Plain words."));
}

#[test]
fn chatgpt_status_and_expiry() {
    let env = Env::new();
    let a = env.accounts();
    let mut c = Credentials {
        access: fake_jwt("acc", "x"),
        refresh: "r".into(),
        expires: 1,
        account_id: "acc".into(),
        invalid: false,
    };
    c.save(env.store.as_ref()).unwrap();
    assert_eq!(
        a.status(ProviderKind::ChatgptAccount),
        ProviderStatus::SignedIn,
        "an expired access token auto-refreshes"
    );
    c.invalid = true;
    c.save(env.store.as_ref()).unwrap();
    assert_eq!(
        a.status(ProviderKind::ChatgptAccount),
        ProviderStatus::TokenExpired
    );
}

#[test]
fn chatgpt_sign_in_through_accounts_and_generate() {
    let access = fake_jwt("acc_7", "n");
    let a2 = access.clone();
    let mock = Mock::start(move |r| match r.route().as_str() {
        "POST /api/accounts/deviceauth/usercode" => Resp::json(
            200,
            json!({"device_auth_id":"d","user_code":"CODE-1","interval":0}),
        ),
        "POST /api/accounts/deviceauth/token" => {
            Resp::json(200, json!({"authorization_code":"c","code_verifier":"v"}))
        }
        "POST /oauth/token" => Resp::json(
            200,
            json!({"access_token": a2, "refresh_token":"r","expires_in":3600}),
        ),
        "POST /codex/responses" => Resp::sse(sse(&[
            (
                "",
                json!({"type":"response.output_text.delta","delta":"hi"}),
            ),
            (
                "",
                json!({"type":"response.completed","response":{"status":"completed"}}),
            ),
        ])),
        _ => Resp::not_found(),
    });
    let env = Env::new();
    let mut e = env.endpoints();
    e.chatgpt = mock.base.clone();
    e.chatgpt_oauth.auth_base = mock.base.clone();
    e.chatgpt_oauth.min_poll_interval = std::time::Duration::from_millis(5);
    let mut a = env.accounts_with(e);
    let code = a.start_chatgpt_device_sign_in().unwrap();
    assert_eq!(code.user_code, "CODE-1");
    a.finish_chatgpt_device_sign_in(&code, &CancelFlag::new())
        .unwrap();
    assert_eq!(
        a.status(ProviderKind::ChatgptAccount),
        ProviderStatus::SignedIn
    );
    assert!(row(&a, ProviderKind::ChatgptAccount).enabled);
    let p = a.pick(Some(ProviderKind::ChatgptAccount)).unwrap();
    assert_eq!(
        p.generate(GenRequest::new("s", "u"), &mut |_| {})
            .unwrap()
            .text,
        "hi"
    );
    assert_eq!(
        mock.last("POST /codex/responses")
            .header("chatgpt-account-id"),
        Some("acc_7")
    );

    a.sign_out(ProviderKind::ChatgptAccount).unwrap();
    assert_eq!(
        a.status(ProviderKind::ChatgptAccount),
        ProviderStatus::NotSet
    );
    assert!(Credentials::load(env.store.as_ref()).unwrap().is_none());
}

#[test]
fn blyg_server_uses_the_owner_token() {
    let mock = Mock::start(|r| match r.route().as_str() {
        "POST /api/items/I1/generate" => {
            Resp::json(200, json!({"text":"t","model":"claude-opus-5"}))
        }
        _ => Resp::not_found(),
    });
    let env = Env::new();
    let mut e = env.endpoints();
    e.blyg_base_url = Some(mock.base.clone());
    let mut a = env.accounts_with(e);
    assert_eq!(a.status(ProviderKind::BlygServer), ProviderStatus::NotSet);
    env.store.set(&mock.base, "owner-tok").unwrap();
    assert_eq!(a.status(ProviderKind::BlygServer), ProviderStatus::SignedIn);
    assert!(
        matches!(
            a.pick(Some(ProviderKind::BlygServer)),
            Err(AiError::NotConfigured(_))
        ),
        "off by default"
    );
    a.sign_in(ProviderKind::BlygServer, Credential::None)
        .unwrap();
    let p = a.pick(Some(ProviderKind::BlygServer)).unwrap();
    let mut r = GenRequest::new("", "");
    r.server_scope = Some(ServerScope {
        item_id: "I1".into(),
        scope: 0,
    });
    assert_eq!(p.generate(r, &mut |_| {}).unwrap().text, "t");
    assert_eq!(
        mock.last("POST /api/items/I1/generate")
            .header("authorization"),
        Some("Bearer owner-tok")
    );
    // Signing out of the server provider leaves the owner token alone.
    a.sign_out(ProviderKind::BlygServer).unwrap();
    assert_eq!(
        env.store.get(&mock.base).unwrap().as_deref(),
        Some("owner-tok")
    );
}
