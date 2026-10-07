# AI helpers

AI is optional and **off until you turn it on**: Burrow uses no provider, not
even an installed `claude` or `codex` CLI, until you enable one (`ai-enable` in
the config file) or sign in to one in Settings. Generation happens while you
write, you review it, and generated text is always disclosed when you
publish.

## Providers

You use your own accounts:

| Provider | How you sign in |
|---|---|
| Anthropic API | API key (stored in the Keychain) |
| OpenAI API | API key (stored in the Keychain) |
| Cloudflare Workers AI | Account ID and API token. Defaults to Gemma 4. |
| Local Claude Code or Codex | Uses your installed `claude` or `codex` CLI and whatever account it's signed in to |
| Your blyg server | Its `/generate` endpoint, if enabled |
| ChatGPT account | Sign in with ChatGPT. **Unofficial:** this uses the same sign-in as OpenAI's Codex CLI, which OpenAI doesn't offer to third-party apps. It may break or be blocked at any time. |

**Not offered:** signing in with a claude.ai (Pro/Max) subscription.
Anthropic's terms don't allow third-party apps to use claude.ai logins. To use
a Claude subscription, install Claude Code and pick the local Claude Code
provider.

## Connecting and generating

1. Turn a provider on. Either open **Settings › AI** (⌘, then "AI accounts…")
   and switch on Claude Code or Codex, paste an API key, or sign in, or add it to
   the config file:

   ```
   ai-enable = codex
   ai-enable = claude-code
   ai-provider = codex        # which one ⌘G uses (optional)
   ```

   API keys go straight to the Keychain and are never shown again: Settings
   shows "key saved" and a Remove button.
2. In a post, write a gap: `[TK]one sentence on why, in my voice[/TK]`.
3. With the caret inside it, press **⌘G**. The status bar shows
   `generating… (codex)`; **esc** cancels. The answer is written in place as
   `[TK]instruction[=]output[/TK]`. Press ⌘G inside it again to regenerate.

The config keys `ai-model` and `ai-provider-model` pick models, and
`ai-style-prompt` adds your own instructions to every prompt ("Write plainly.
British spelling."). See [Configuration](config.md#ai-provider).

## The helpers

⌘G anywhere outside a gap opens the helpers:

| Helper | What it does |
|---|---|
| Fill a gap here | Inserts an empty `[TK][/TK]` at the caret for you to fill in. |
| Shorten to fit 1000 | Also **⇧⌘G**. Rewrites a fragment to fit, shown as a word diff: ⏎ accepts, esc rejects. |
| Continue this thought | Writes on from the caret. |
| Outline a thread | Drafts the outline of a new thread. |
| Proofread | Typos and grammar only, applied as a plain edit. |
| AI reply | From a post you're reading: a reply stub with a first draft. |

Each generating helper writes its text as a TK span, so the preview tints it
and it's disclosed like any gap.

## Disclosure

Generated text is always disclosed as generated when you publish. Burrow
records which model wrote each gap and sends it to your blyg with the text
(the provenance extension in [Server requirements](server.md)). If your blyg
doesn't have that extension, Burrow warns you before publishing generated
text, and Cancel is the default. Proofreading isn't generated prose and isn't
disclosed.

How the providers, sign-in, prompts and provenance work inside is in
[blyg-ai](dev/ai.md).
