//! The one table of configuration keys. Defaults, validation, the
//! `+show-config --default --docs` output and the migration all read from
//! here, so the docs and the defaults can't drift apart.

/// What a key's value must look like.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ValueKind {
    /// Free text (fonts, prompts, ids, models).
    Text,
    /// An `http(s)://` URL with a host.
    Url,
    /// `true` or `false`.
    Bool,
    /// A number, clamped to `min..=max` (with a warning).
    Number { min: f32, max: f32 },
    /// One of a fixed set of words (case-insensitive, stored lowercase).
    Choice(&'static [&'static str]),
    /// A key combination such as `ctrl+alt+b`.
    Hotkey,
    /// A path; `~/` is expanded, relative paths are relative to the file that
    /// names them. A leading `?` marks the file optional.
    Path,
    /// `<provider>=<model>`.
    ProviderModel,
}

/// One configuration key.
#[derive(Debug, Clone, Copy)]
pub struct KeySpec {
    pub name: &'static str,
    pub kind: ValueKind,
    /// The default as it would be written in the file. `None` = unset.
    pub default: Option<&'static str>,
    /// For repeatable keys: the default list (empty for none).
    pub default_list: &'static [&'static str],
    /// Repeatable keys collect every value into a list.
    pub repeatable: bool,
    /// Plain text, one paragraph per line; shown as `#` comments.
    pub docs: &'static str,
}

/// Every AI provider name the config understands (`none` only for `ai-provider`).
pub const AI_PROVIDERS: &[&str] = &[
    "claude-code",
    "codex",
    "chatgpt",
    "openai",
    "anthropic",
    "cloudflare",
    "server",
];

const AI_PROVIDER_CHOICES: &[&str] = &[
    "claude-code",
    "codex",
    "chatgpt",
    "openai",
    "anthropic",
    "cloudflare",
    "server",
    "none",
];

const fn key(name: &'static str, kind: ValueKind, default: Option<&'static str>) -> KeySpec {
    KeySpec {
        name,
        kind,
        default,
        default_list: &[],
        repeatable: false,
        docs: "",
    }
}

const fn docs(mut k: KeySpec, docs: &'static str) -> KeySpec {
    k.docs = docs;
    k
}

const fn list(mut k: KeySpec, default_list: &'static [&'static str]) -> KeySpec {
    k.repeatable = true;
    k.default_list = default_list;
    k
}

pub const KEYS: &[KeySpec] = &[
    docs(
        key("blyg-url", ValueKind::Url, None),
        "The blyg this app writes to, e.g. https://blyg.example.com. There is no default: \
         with no blyg-url the app starts by asking you to connect one.\n\
         The owner token for this blyg is kept in the macOS Keychain, never in this file.",
    ),
    docs(
        key(
            "theme",
            ValueKind::Choice(&["system", "light", "dark"]),
            Some("system"),
        ),
        "Colour theme: system (follow macOS light/dark), light or dark.",
    ),
    docs(
        key(
            "layout",
            ValueKind::Choice(&["side", "stacked"]),
            Some("side"),
        ),
        "Window layout: side puts the list to the left of the editor; stacked puts it above.",
    ),
    // --- buttons ---
    docs(
        key("show-buttons", ValueKind::Bool, Some("true")),
        "Show the toolbar buttons in the title bar (New, Make draft, Publish, the views, \
         Versions, Generate, Quick capture) and the Scratch · Draft · Publish row in quick \
         capture. Every button's tooltip shows its shortcut. false keeps the window \
         keyboard-only and minimal. Settings (⌘,) toggles it.",
    ),
    // --- auto-update ---
    docs(
        key(
            "auto-update",
            ValueKind::Choice(&["install", "notify", "off"]),
            Some("install"),
        ),
        "Updates from the project's GitHub releases. install downloads a new release in the \
         background, checks its signature, and shows \"Restart to update\" in the status bar \
         (quitting installs it too); notify only says a new release is available; off never \
         checks on its own. Blygger › Check for Updates… always checks. An update is refused \
         unless it's signed with the project's release key.",
    ),
    docs(
        key("font-family-writing", ValueKind::Text, Some("Literata")),
        "Font for the editor and preview. Run `blygger +list-fonts` to see the choices.",
    ),
    docs(
        key("font-family-ui", ValueKind::Text, Some("Inter")),
        "Font for the list, the omnibar and sheets. Run `blygger +list-fonts` to see the choices.",
    ),
    docs(
        key(
            "font-size",
            ValueKind::Number {
                min: 12.0,
                max: 32.0,
            },
            Some("19"),
        ),
        "Writing font size in points, from 12 to 32. ⌘+ and ⌘− change it and write it back here.",
    ),
    docs(
        key("capture-hotkey", ValueKind::Hotkey, Some("ctrl+alt+b")),
        "Global hotkey for the quick-capture panel. Modifiers are ctrl, alt (option), shift and \
         cmd, joined with +, e.g. ctrl+alt+b or cmd+shift+space.",
    ),
    // --- scratch notes ---
    docs(
        key(
            "capture-default",
            ValueKind::Choice(&["scratch", "draft"]),
            Some("scratch"),
        ),
        "What quick capture keeps when you press esc or ⌘S, or click away: scratch keeps a \
         scratch note that stays on this Mac (never synced, never published, until you make \
         it a draft with ⌘D or publish it with ⌘⏎); draft saves a draft on your blyg.\n\
         In the capture panel, ⌘D always saves a draft and ⌘⏎ always publishes.",
    ),
    docs(
        key(
            "new-note",
            ValueKind::Choice(&["draft", "scratch"]),
            Some("draft"),
        ),
        "What the main window's omnibar creates when ⏎ finds nothing: draft (a draft on your \
         blyg) or scratch (a scratch note that stays on this Mac until ⌘D or ⌘⏎).",
    ),
    docs(
        key(
            "edited-posts",
            ValueKind::Choice(&["top", "stay"]),
            Some("top"),
        ),
        "What happens to a reading-list post when its author edits it: top moves it to the top \
         of the list; stay leaves it where it was.",
    ),
    docs(
        key(
            "ai-provider",
            ValueKind::Choice(AI_PROVIDER_CHOICES),
            Some("none"),
        ),
        "The AI provider the writing helpers use by default: claude-code or codex (your locally \
         installed CLI), chatgpt, openai, anthropic, cloudflare, server (your blyg's own \
         generate endpoint), or none.\n\
         none picks the first ready provider listed in ai-enable, in the order claude-code, \
         anthropic, chatgpt, openai, codex, cloudflare, server.\n\
         API keys and sign-ins live in the macOS Keychain, never in this file.",
    ),
    docs(
        key("ai-model", ValueKind::Text, None),
        "Model for ai-provider. Unset uses that provider's default.",
    ),
    docs(
        list(
            key("ai-provider-model", ValueKind::ProviderModel, None),
            &[],
        ),
        "Model for one particular provider, as provider=model, e.g. \
         anthropic=claude-sonnet-5. Repeat the key for more providers.",
    ),
    docs(
        list(key("ai-enable", ValueKind::Choice(AI_PROVIDERS), None), &[]),
        "Providers that are switched on. Repeat the key for each one. None by default: \
         Blygger uses no AI (not even a locally installed claude or codex CLI) until you \
         enable a provider here or sign in to one in Settings, which updates this list. \
         An empty `ai-enable =` switches every provider off again.",
    ),
    docs(
        key("cloudflare-account-id", ValueKind::Text, None),
        "Cloudflare account ID for Workers AI. Not a secret; the API token is in the Keychain.",
    ),
    docs(
        key("ai-style-prompt", ValueKind::Text, None),
        "Extra instructions appended to every generation prompt, e.g. \
         \"Write plainly. British spelling.\"",
    ),
    docs(
        key(
            "ai-disclose",
            ValueKind::Choice(&["always"]),
            Some("always"),
        ),
        "Generated text is always disclosed as generated when published. This is fixed: \
         always is the only value.",
    ),
    docs(
        key("tutorial-on-launch", ValueKind::Bool, Some("false")),
        "Show the interactive tutorial every time Blygger opens. The very first launch always \
         shows it; after that it follows this setting.",
    ),
    docs(
        list(key("config-file", ValueKind::Path, None), &[]),
        "Load another config file after this one. Relative paths are relative to this file. \
         A leading ? makes the file optional (no error if it's missing), e.g. \
         config-file = ?local.config. Repeatable.",
    ),
];

pub fn spec(name: &str) -> Option<&'static KeySpec> {
    KEYS.iter().find(|k| k.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_consistent() {
        let mut names: Vec<_> = KEYS.iter().map(|k| k.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), KEYS.len(), "duplicate key");
        for k in KEYS {
            assert!(!k.docs.is_empty(), "{} has no docs", k.name);
            assert!(
                k.name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{} isn't kebab-case",
                k.name
            );
            assert!(!(k.repeatable && k.default.is_some()), "{}", k.name);
            if let (ValueKind::Choice(opts), Some(d)) = (k.kind, k.default) {
                assert!(opts.contains(&d), "{} default {d} not a choice", k.name);
            }
        }
        assert!(
            spec("blyg-url").unwrap().default.is_none(),
            "no default blyg"
        );
    }
}
