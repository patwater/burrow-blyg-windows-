<!-- Generated from crates/blyg-core/src/config/keys.rs by scripts/gen-docs.sh.
     Don't edit it by hand: cargo test fails when it's out of date. -->

### `blyg-url`

- **Values:** an `http(s)://` address
- **Default:** unset

The blyg this app writes to, e.g. https://blyg.example.com. There is no default: with no blyg-url the app starts by asking you to connect one.

The owner token for this blyg is kept in the macOS Keychain, never in this file.


### `theme`

- **Values:** `system`, `light`, `dark`, a built-in theme (`cutaway`, `kumiko`, `shola`, `fortress`, `portolan`, `aizome`, `saltspace`, `konkan`) or a theme file's name (see Themes)
- **Default:** `system`

Colour theme: system (follow macOS light and dark), light (Paper), dark (Ink), one of the built-in themes (woody: cutaway, kumiko, shola, fortress; oceanic: portolan, aizome, saltspace, konkan), or the name of a theme file in ~/.config/blygger/themes/. `blygger +list-themes` lists them all, and `blygger +copy-theme &lt;name>` copies a built-in there to edit. Theme files reload when you save them.


### `theme-dark`

- **Values:** `system`, `light`, `dark`, a built-in theme (`cutaway`, `kumiko`, `shola`, `fortress`, `portolan`, `aizome`, `saltspace`, `konkan`) or a theme file's name (see Themes)
- **Default:** unset

The theme to use while macOS is dark, whatever theme says, e.g. theme = cutaway with theme-dark = fortress. Unset, theme = system switches to dark and every other theme stays as it is.


### `layout`

- **Values:** `side`, `stacked`
- **Default:** `side`

Window layout: side puts the list to the left of the editor; stacked puts it above.


### `show-buttons`

- **Values:** `true` or `false`
- **Default:** `true`

Show the toolbar buttons in the title bar (New, Make draft, Publish, the views, Versions, Generate, Quick capture) and the Scratch · Draft · Publish row in quick capture. Every button's tooltip shows its shortcut. false keeps the window keyboard-only and minimal. Settings (⌘,) toggles it.


### `spellcheck`

- **Values:** `true` or `false`
- **Default:** `true`

Check spelling while you type, with the macOS spell checker (your system languages and the words you've taught it). Misspelled words get a red wavy underline; right-click one for suggestions, Learn Spelling and Ignore. Code, links, addresses and blyg markup are never checked. Edit › Spelling › Check Spelling While Typing toggles it.


### `auto-update`

- **Values:** `install`, `notify`, `off`
- **Default:** `install`

Updates from the project's GitHub releases. install downloads a new release in the background, checks its signature, and shows "Restart to update" in the status bar (quitting installs it too); notify only says a new release is available; off never checks on its own. Burrow › Check for Updates… always checks. An update is refused unless it's signed with the project's release key.


### `open-links`

- **Values:** `app`, `browser`
- **Default:** `app`

Where a link clicked in a post opens: app opens it in Burrow's browser pane (from the right, over the reading view; esc closes it), browser opens your default browser. ⌘-click opens the pane over the whole reading area, and ⌥-click does the other one (the default browser with app, the pane with browser).


### `content-blocking`

- **Values:** `true` or `false`
- **Default:** `true`

Block ads and trackers in the browser pane, with uBlock Origin's default filter lists (downloaded about once a week; a small built-in list until then). The shield button turns it off for one site. false turns it off everywhere.


### `font-family-writing`

- **Values:** text
- **Default:** `Literata`

Font for the editor and preview. Run `blygger +list-fonts` to see the choices.


### `font-family-ui`

- **Values:** text
- **Default:** `Inter`

Font for the list, the omnibar and sheets. Run `blygger +list-fonts` to see the choices.


### `font-size`

- **Values:** a number from 12 to 32
- **Default:** `19`

Writing font size in points, from 12 to 32. ⌘+ and ⌘− change it and write it back here.


### `capture-hotkey`

- **Values:** a key combination, such as `ctrl+alt+b`
- **Default:** `ctrl+alt+b`

Global hotkey for the quick-capture panel. Modifiers are ctrl, alt (option), shift and cmd, joined with +, e.g. ctrl+alt+b or cmd+shift+space.


### `capture-default`

- **Values:** `scratch`, `draft`
- **Default:** `scratch`

What quick capture keeps when you press esc or ⌘S, or click away: scratch keeps a scratch note that stays on this Mac (never synced, never published, until you make it a draft with ⌘D or publish it with ⌘⏎); draft saves a draft on your blyg.

In the capture panel, ⌘D always saves a draft and ⌘⏎ always publishes.


### `new-note`

- **Values:** `draft`, `scratch`
- **Default:** `draft`

What the main window's omnibar creates when ⏎ finds nothing: draft (a draft on your blyg) or scratch (a scratch note that stays on this Mac until ⌘D or ⌘⏎).


### `edited-posts`

- **Values:** `top`, `stay`
- **Default:** `top`

What happens to a reading-list post when its author edits it: top moves it to the top of the list; stay leaves it where it was.


### `ai-provider`

- **Values:** `claude-code`, `codex`, `chatgpt`, `openai`, `anthropic`, `cloudflare`, `server`, `none`
- **Default:** `none`

The AI provider the writing helpers use by default: claude-code or codex (your locally installed CLI), chatgpt, openai, anthropic, cloudflare, server (your blyg's own generate endpoint), or none.

none picks the first ready provider listed in ai-enable, in the order claude-code, anthropic, chatgpt, openai, codex, cloudflare, server.

API keys and sign-ins live in the macOS Keychain, never in this file.


### `ai-model`

- **Values:** text
- **Default:** unset

Model for ai-provider. Unset uses that provider's default.


### `ai-provider-model`

- **Values:** `provider=model`
- **Default:** none
- **Repeatable:** yes, one value per line

Model for one particular provider, as provider=model, e.g. anthropic=claude-sonnet-5. Repeat the key for more providers.


### `ai-enable`

- **Values:** `claude-code`, `codex`, `chatgpt`, `openai`, `anthropic`, `cloudflare`, `server`
- **Default:** none
- **Repeatable:** yes, one value per line

Providers that are switched on. Repeat the key for each one. None by default: Burrow uses no AI (not even a locally installed claude or codex CLI) until you enable a provider here or sign in to one in Settings, which updates this list. An empty `ai-enable =` switches every provider off again.


### `cloudflare-account-id`

- **Values:** text
- **Default:** unset

Cloudflare account ID for Workers AI. Not a secret; the API token is in the Keychain.


### `ai-style-prompt`

- **Values:** text
- **Default:** unset

Extra instructions appended to every generation prompt, e.g. "Write plainly. British spelling."


### `ai-disclose`

- **Values:** `always`
- **Default:** `always`

Generated text is always disclosed as generated when published. This is fixed: always is the only value.


### `tutorial-on-launch`

- **Values:** `true` or `false`
- **Default:** `false`

Show the interactive tutorial every time Burrow opens. The very first launch always shows it; after that it follows this setting.


### `config-file`

- **Values:** a file path (`~/` is expanded; a leading `?` makes it optional)
- **Default:** none
- **Repeatable:** yes, one value per line

Load another config file after this one. Relative paths are relative to this file. A leading ? makes the file optional (no error if it's missing), e.g. config-file = ?local.config. Repeatable.
