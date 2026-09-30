# blyg-ai: providers, sign-in and prompts

`crates/blyg-ai` is everything AI in Blygger Desktop: the provider
abstraction, sign-in and credentials, and the TK / helper prompts. It
doesn't depend on the UI. The app calls it on a background thread.

## Provider contract

```rust
pub trait Provider: Send + Sync {
    fn kind(&self) -> ProviderKind;
    fn generate(&self, req: GenRequest, on_delta: &mut dyn FnMut(&str)) -> Result<GenResult>;
    fn list_models(&self) -> Result<Vec<ModelInfo>>; // Ok(vec![]) if unsupported
}
```

- **Blocking.** Run it on a background thread and forward the deltas to the UI.
- **Streaming.** `on_delta` receives text only. Thinking and reasoning output is never passed through.
- **Cancellation.** `GenRequest.cancel` is a shared `CancelFlag`, checked between SSE events. For CLI bridges, cancelling kills the child process. An HTTP read that is already blocked finishes its current read first (the read timeout is 300 s).
- `GenResult { text, model }`, where `model` is the model that answered. Record it as provenance.

## Providers

| Kind | Auth | Endpoint / command | Default model |
|---|---|---|---|
| `AnthropicApi` | API key (Keychain) | `POST https://api.anthropic.com/v1/messages`, SSE | `claude-opus-5` |
| `OpenaiApi` | API key (Keychain) | `POST https://api.openai.com/v1/responses`, SSE | `gpt-6-astra` |
| `ChatgptAccount` | Sign in with ChatGPT (**unofficial**) | `POST https://chatgpt.com/backend-api/codex/responses`, SSE | `gpt-6-astra` |
| `LocalClaudeCode` | whatever `claude` is signed in to | `claude -p --output-format stream-json …` | CLI default |
| `LocalCodex` | whatever `codex` is signed in to | `codex exec --json --sandbox read-only …` | CLI default |
| `CloudflareWorkersAi` | account id (`cloudflare-account-id` in the config file) + API token (Keychain) | `POST https://api.cloudflare.com/client/v4/accounts/{id}/ai/v1/chat/completions`, SSE | `@cf/google/gemma-4-26b-a4b-it` |
| `BlygServer` | the blyg owner token | `POST {blyg}/api/items/:id/generate {scope}` | the server's `ai_model` |

### Anthropic

The request has `model`, `max_tokens: 16000`, `stream: true`,
`thinking: {type: "adaptive"}`, `system`, and a single user message. It never
sends `budget_tokens`, which Opus 5 rejects. The headers are `x-api-key` and
`anthropic-version: 2023-06-01`. A `stop_reason: "refusal"` becomes
`AiError::Refused("provider declined the request (<category>)")`, which is the
server's wording. `max_tokens` is higher than the server's 4096 because
adaptive thinking spends from it.

The server-side refusal `fallbacks` beta is **not** enabled. The spec asks for
refusals to surface as clean errors, and a fallback would silently change
the model that gets recorded in provenance.

### OpenAI (API key)

The Responses API request has `model`, `instructions` (the system prompt),
`input: [{role: "user", content: [{type: "input_text", text}]}]`,
`stream: true` and `store: false`. Handled events: `response.output_text.delta`,
`response.refusal.delta`, `response.completed|done|incomplete`,
`response.failed`, and `error`. The default model is `gpt-6-astra`. The docs at
developers.openai.com/api/docs/models say: "Start with GPT-6 Astra for complex
reasoning and coding".

### Sign in with ChatGPT (unofficial; it may break)

> **This is unofficial.** OpenAI offers no sanctioned way for third-party apps
> to use a ChatGPT plan. The flow borrows the public OAuth client of OpenAI's
> Codex CLI, the same way the pi coding agent does. OpenAI can change or block
> it at any time. The UI should say this once (`AiConfig::chatgpt_notice_shown`).

Every value below was copied from pi-ai 0.85.1
(`@earendil-works/pi-ai`, github.com/badlogic/pi-mono `packages/ai`):

| What | Value | pi source |
|---|---|---|
| Client id | `app_EMoamEEZ73f0CkXaXp7hrann` | `dist/auth/oauth/openai-codex.js` |
| Authorize | `https://auth.openai.com/oauth/authorize` with `response_type=code`, `client_id`, `redirect_uri`, `scope`, `code_challenge` (S256), `state`, `id_token_add_organizations=true`, `codex_cli_simplified_flow=true`, `originator` | same |
| Scope | `openid profile email offline_access` | same |
| Redirect | `http://localhost:1455/auth/callback` (listener on 127.0.0.1:1455) | same |
| Token | `POST https://auth.openai.com/oauth/token`, form: `authorization_code` (+`code_verifier`, `redirect_uri`) or `refresh_token` | same |
| Device code | `POST /api/accounts/deviceauth/usercode {client_id}` → `{device_auth_id, user_code, interval}`; poll `POST /api/accounts/deviceauth/token {device_auth_id, user_code}` (403/404 = pending) → `{authorization_code, code_verifier}`; exchange with redirect `https://auth.openai.com/deviceauth/callback`; user visits `https://auth.openai.com/codex/device`; 15-minute timeout | same |
| PKCE | 32 random bytes → base64url verifier; challenge = base64url(SHA-256(verifier)) | `dist/auth/oauth/pkce.js` |
| Account id | JWT claim `https://api.openai.com/auth`.`chatgpt_account_id` | `openai-codex.js` |
| Endpoint | `https://chatgpt.com/backend-api/codex/responses` | `dist/api/openai-codex-responses.js` |
| Headers | `Authorization: Bearer <access>`, `chatgpt-account-id`, `originator`, `User-Agent`, `OpenAI-Beta: responses=experimental`, `accept: text/event-stream` | same |
| Body | `{model, store: false, stream: true, instructions, input, text: {verbosity}, include: ["reasoning.encrypted_content"], tool_choice: "auto", parallel_tool_calls: true}` | same |

Where we differ from pi:

- `originator` is `blygger`, not `pi`.
- `text.verbosity` is `medium` (pi uses `low`) because we generate prose.
- There's no WebSocket transport, only SSE.

Tokens are stored as JSON under the Keychain account `blygger-ai.chatgpt-oauth`.
They are refreshed when they're within 60 s of expiry, and once more on a 401.
If the refresh itself is rejected, the stored credentials are marked
`invalid`, the status becomes `TokenExpired`, and the user has to sign in again.

### Local Claude Code (the supported way to use a Claude subscription)

`claude -p --output-format stream-json --verbose --include-partial-messages
--tools "" --strict-mcp-config --permission-mode dontAsk
--disable-slash-commands --no-session-persistence --system-prompt <system>
[--model <m>]`. The prompt goes on stdin, and the working directory is a fresh
empty temp dir.

- `--tools ""` removes every built-in tool.
- `--strict-mcp-config` without `--mcp-config` loads no MCP servers.
- `dontAsk` denies anything that would prompt.

We don't use `--bare`, because bare mode ignores the subscription login.
Sources: code.claude.com/docs/en/headless and `claude --help` (v2.1.282).

Text comes from `stream_event` `text_delta`s. The final
`{"type":"result","is_error":…,"result":…}` line is authoritative. An
`is_error` result or a non-zero exit becomes `AiError::CliFailed`.

The binary is looked up on `$PATH`, then in `~/.claude/local`, `~/.local/bin`,
`~/.npm-global/bin`, `~/.bun/bin`, `~/.volta/bin`, `~/.cargo/bin`,
`/opt/homebrew/bin`, `/usr/local/bin` and `/usr/bin`. The child's `PATH` is set
to those same directories so that node shims can find `node`.

### Local Codex

`codex exec --json --sandbox read-only --skip-git-repo-check --ephemeral
--color never -C <empty temp dir> [-m <m>] -`, with the prompt on stdin.
Codex has no system-prompt flag, so the system prompt is prepended under a
heading. The bridge parses `item.*` events whose `item.type` is
`agent_message`, plus `turn.failed` and `error`. Sources: `codex exec --help`
(codex-cli 0.155.1) and learn.chatgpt.com/docs/non-interactive-mode.

### Cloudflare Workers AI

This uses the OpenAI-compatible chat completions endpoint
(developers.cloudflare.com/workers-ai/configuration/open-ai-compatibility/)
with `Authorization: Bearer <token>`. The request body is
`{model, messages: [system, user], stream: true, max_completion_tokens}`.
Gemma 4 reasons by default (`chat_template_kwargs.enable_thinking` defaults to
true, per the model's input schema). The published output schema doesn't say
where the reasoning text appears, so every known form is dropped:
`delta.reasoning_content`, `delta.reasoning`, and `<think>…</think>` inside
`content`, even when a tag is split across chunks. `list_models()` calls
`GET /accounts/{id}/ai/models/search?task=Text Generation` and puts Gemma 4
first. The provenance string is the full `@cf/…` id.

### Blyg server

blyg-core's `Api` has no method for `/generate`, so this is a small direct
call. The server builds its own prompt with its own key, model and style
prompt. It **splices the output into its working copy and records provenance
itself**, so afterwards the app must re-pull the item rather than splice
locally. It needs `GenRequest.server_scope = Some(ServerScope {item_id, scope})`.
It's off by default, because the Worker also needs `AI_PROVIDER_KEY`.

### Not built: claude.ai (Pro/Max) login

Anthropic's terms prohibit third-party apps from offering claude.ai login or
using subscription limits. `LocalClaudeCode` is the supported route.

## Accounts

`Accounts::load(ConfigStore, Arc<dyn TokenStore>)` reads its settings from the
Ghostty-style config file (`blygger +show-config --default --docs` lists them):

| Key | Meaning |
|---|---|
| `ai-provider` | the default provider (`none` = the first ready one in `ai-enable`) |
| `ai-model` | the model for `ai-provider` |
| `ai-provider-model` | `provider=model` for any other provider (repeatable) |
| `ai-enable` | the providers that are switched on (repeatable). **Empty by default**: AI is opt-in, so no provider is used, not even a locally installed `claude` or `codex` CLI, until the user enables it or signs in |
| `cloudflare-account-id` | the Workers AI account (not a secret) |
| `ai-style-prompt` | extra instructions appended to every generation prompt |

Signing in or out, `set_enabled` and `set_default` write these keys back,
keeping the rest of the file (comments included). The config file never holds
secrets. The "Sign in with ChatGPT is unofficial" notice is app state, not
configuration: `blyg_core::state::AppState::chatgpt_notice_shown` in
`state.json` in the data dir.

Secrets are stored through blyg-core's `TokenStore` under these accounts:

- `blygger-ai.anthropic-api-key`
- `blygger-ai.openai-api-key`
- `blygger-ai.chatgpt-oauth`
- `blygger-ai.cloudflare-api-token`

The blyg server uses the existing owner token.

- `status(kind)` returns one of `SignedIn`, `NotSet`, `CliFound(path)`, `CliNotFound` or `TokenExpired`.
- `rows()` returns one `AccountRow` per provider, for Settings › AI accounts.
- `sign_in(kind, Credential::{ApiKey, Cloudflare{..}, None})` and `sign_out(kind)`.
- For ChatGPT, use `start_chatgpt_browser_sign_in()`, open `login.url` with `open_in_browser`, then call `finish_chatgpt_browser_sign_in(&login, cancel, timeout)`. The paste fallback is `finish_chatgpt_pasted`. The device-code flow is `start_chatgpt_device_sign_in()` followed by `finish_chatgpt_device_sign_in`.
- `pick(Option<ProviderKind>)` returns the chosen provider if it's enabled and ready. With no choice it uses the default: the configured `ai-provider` if it's usable, otherwise the first ready provider in the order local Claude Code, Anthropic, ChatGPT, OpenAI, local Codex, Cloudflare, blyg server.
- Nothing is on by default (`ai-enable` is empty). A found `claude`/`codex` CLI shows as `CliFound` but isn't used until the user enables it; API providers switch on when the user signs in.

## Prompts (`prompts.rs`)

**TK parity.** `SYSTEM_PROMPT`, `tk_user_content`, `mark_document`,
`parse_scopes` and `set_scope_output` mirror the reference Worker's `worker/src/ai/provider.ts` and
`tk.ts` byte for byte. `tk_job(content_md, index, resolve, style_prompt)`
builds the prompt the way `tk-generate.ts::runGenerateScope` does:

- the style prompt is appended as `\n\n<style>`;
- the scope's current output is used when regenerating;
- sources are resolved through the caller's `resolve(id)`, which returns a local published fragment;
- the working copy is included with the active scope wrapped in `<<<TK-SCOPE>>>…<<<END-TK-SCOPE>>>`.

`tests/fixtures/provider_ts_parity.json` is generated from the server's own
TypeScript by `node crates/blyg-ai/tests/fixtures/gen_parity.mjs`. Re-run the
script whenever the server's prompt changes.

**Helpers.** Every generating helper is itself a TK generation that goes
through the same prompt, and it returns `HelperOutput { instruction, output,
insert, model }`. `insert` is `wrap_tk(instruction, output)`, which is
`[TK]<instruction>[=]<output>[/TK]`, so the text is disclosed like any other
TK. `wrap_tk` strips any TK tokens from the model output first.

| Helper | Function | Disclosed | Notes |
|---|---|---|---|
| Fill a gap / regenerate | `tk_job` | yes | server parity |
| Shorten to fit 1000 | `shorten_to_fit` | yes | The insert replaces the whole working copy. It aims for 900 characters and checks `published_len(insert) <= 1000` (UTF-16, TK-stripped). If the result is over, it retries once at 700, revising the over-long attempt. It returns `fits`, `len` and `attempts`. |
| Continue this thought | `continue_thought(doc, at)` | yes | The scope is inserted at byte offset `at`. |
| Outline a thread | `outline_thread` | yes | The insert is the new thread's body. |
| Reply to a reading item | `reply_draft(item)` | yes | The post is passed as source material, labelled with its `remote_id`. |
| Proofread | `proofread` | **no** (`disclose: false`) | Uses its own prompt and returns `Suggestion {start, end, original, replacement, reason}` as byte offsets into the original text, always on char boundaries. Suggestions that can't be located, change nothing or overlap are dropped. `apply_suggestions` applies them. |

## Provenance

The app records `{sources: [{id, version}], model, at}` for each scope it
generates locally, with `Backend::save_with_provenance(id, text, scopes)`: one
`Option<ScopeProvenance>` per TK scope, in order (`None` = written by hand).
blyg-core then keeps it attached to its scope as the text is edited
(`blyg_core::tk::remap`) and pushes text and provenance together with
`PUT /api/items/:id/tk-provenance` whenever the scopes move, because the
server keys provenance by position (docs/SPEC.md § Client-recorded
provenance; docs/CORE.md § Provenance). That needs the provenance extension in
`docs/SERVER.md` (see also `docs/BLYGGER-SPEC-DIGEST.md` §3.6). On a Worker
without it (`LiveBackend::provenance_available()` is false), text generated
locally would publish without disclosure, so the app warns before publishing it.

## In the app (`crates/blyg-app/src/ai/`)

**Connect.** AI is off until a provider is enabled or signed in. Either use
**Settings › AI** (⌘, → "AI accounts…"), or add `ai-enable = codex` /
`ai-enable = claude-code` (and optionally `ai-provider`) to the config file.
Settings › AI lists every provider with its status (enabled, signed in,
installed · off, not installed, not set, sign in again), switches the CLI
bridges and the blyg server on and off, picks the provider ⌘G uses ("Use for
⌘G", written as `ai-provider`) and its model (`ai-model` /
`ai-provider-model`), takes API keys and the Cloudflare account ID + token,
and runs Sign in with ChatGPT (the "unofficial" note shows once; the browser
PKCE flow runs off the UI thread). API keys go straight to the Keychain: the
field is cleared at once, and a stored key only ever shows as "key saved"
with a Remove button. All config writes go through `Accounts`, which uses
blyg-core's config edit module on the app's one config store
(`ai::with_accounts` lends it). There is no claude.ai login.

**Generate.** With the caret inside `[TK]instruction[/TK]` (or
`[TK]instruction[=]old[/TK]` to regenerate), **⌘G** builds the prompt with
`prompts::tk_job` (server parity; `![[id]]` sources resolve from your own
posts and imported reading items, never fetched), picks the provider
(`Accounts::default_provider`), and runs it on its own thread. The status
bar shows `generating… (codex) · esc cancels`. **esc** sets the cancel flag
(a CLI child is killed) and the result is dropped; so is one that arrives
after the timeout (240 s). The output is written as
`[TK]instruction[=]output[/TK]` (TK tokens stripped from it) into the scope
as it is *now*, found by index and instruction; if it was edited away,
nothing is written. The blyg-server provider instead asks the Worker to
generate, then re-pulls (the Worker records provenance itself).

**Provenance.** Every write of generated text goes through
`Backend::save_with_provenance` with one entry per scope: the tracked
provenance carried to the new text (`tk::remap`), plus `{model, sources,
at}` for the new scope. An ordinary edit that changes the set of TK scopes
also pushes the carried-over array (not a plain save). A scope rewritten
entirely by hand becomes `null` (`tk::remap`).

**Helpers.** ⌘G with no TK under the caret opens a palette: fill a gap here
(inserts `[TK][/TK]`), shorten to fit 1000 (also **⇧⌘G**: a side-by-side
word diff you accept with ⏎ or reject with esc), continue this thought
(inserted at the caret as a TK), outline a thread (a new thread draft),
proofread (suggestions applied as a plain edit, **not** disclosed), and reply
to a reading item (`MainView::ai_reply_to`, a hook for the reading screen).

**Disclosure.** Before publishing an item with app-generated scopes, if the
server answered 404 to the provenance endpoint
(`Backend::provenance_available`), the app warns "This will publish without
AI disclosure"; ⏎ and esc cancel, P publishes anyway.
`BLYGGER_FAKE_NO_PROVENANCE=1` simulates such a server in fake mode.

**Errors** are toasts that say what to do: no provider ready (open
Settings › AI), CLI not installed, not signed in / key rejected / sign-in
expired, timeout, and a CLI's non-zero exit. Anything that looks like a
key or token is redacted from provider messages first.

**Not done:** the editor doesn't tint TK output. gpui-base's `Textarea`
(`InputBaseState<TextareaMode>`) has no highlight or decoration API; text
decorations exist only on the code-editor mode (`EditorState`), which would
mean replacing the editor.

`BLYGGER_DEMO=ai-fill|ai-palette|ai-shorten|ai-settings` walks through these
in fake mode; with `BLYGGER_TIMING=1` the app prints `ai fill ok provider=…
model=… chars=…` or `ai failed: …`.
