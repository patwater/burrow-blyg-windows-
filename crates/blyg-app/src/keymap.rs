//! Every key binding the app has, in one table: the key, where it applies,
//! what it does, and its menu. `bind_keys` registers the table,
//! `blygger +list-keybinds` prints it, and the tests below refuse clashes.
//!
//! Where a binding applies (`Scope`):
//! - `Global`: anywhere in the app.
//! - `Main`: the main window (key context `Blygger`). gpui-base's text
//!   inputs sit *inside* it with their own, deeper `Input` context, so an
//!   `Input` binding on the same key would win while an input has focus.
//! - `MainInput`: `Blygger > Input`, a binding that deliberately out-ranks
//!   gpui-base's own binding of the same key inside our inputs (⌘⏎, which
//!   gpui-base binds to "submit").

use gpui_kit::{Action, App, KeyBinding};

use crate::app::ShowCapture; // --- buttons ---

use crate::ai::{AiGenerate, AiShorten}; // --- AI ---

use crate::app::{
    Disconnect, FakeConflict, FakeToggleOffline, FocusSearch, FontBigger, FontReset, FontSmaller,
    NewDraft, OpenConfigFile, OpenPermalink, OpenSettings, Publish, Quit, ReloadConfig, ToggleKind,
    TogglePreview,
};
// --- scratch notes ---
use crate::app::scratch::{KeepCapture, MakeDraft};
// --- full editor ---
use crate::app::studio::{ViewSplit, ViewStudio, ViewWrite};
// --- onboarding ---
use crate::app::onboarding::{ShowTutorial, TutorialBack, TutorialNext};
// --- reading & versions ---
use crate::app::reading::{
    QuotePicker, ShowMentions, ShowReading, ShowSubscriptions, ShowVersions, SiteSettings,
    SubscribeTo,
};

// --- profiles ---
use crate::app::profiles::{MyProfile, OpenProfile, ShowProfile};
// --- delete & withdraw ---
use crate::app::discard::{DeleteDraft, Withdraw};
// --- auto-update ---
use crate::update::CheckForUpdates;

pub const MAIN: &str = crate::app::CONTEXT;
/// Our own inputs inside the main window (deeper than gpui-base's `Input`).
pub const MAIN_INPUT: &str = "Blygger > Input";
/// A context that never matches: bindings here exist only so the native
/// menu shows the right key equivalent (see `Keybind::menu_key`).
const MENU_ONLY: &str = "BlyggerMenuKeyEquivalent";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Global,
    Main,
    MainInput,
}

impl Scope {
    fn context(self) -> Option<&'static str> {
        match self {
            Scope::Global => None,
            Scope::Main => Some(MAIN),
            Scope::MainInput => Some(MAIN_INPUT),
        }
    }

    /// Where the key is live, for clash checks: everything in the main
    /// window counts as one place.
    #[cfg(test)]
    fn place(self) -> &'static str {
        "main window"
    }
}

pub struct Keybind {
    /// GPUI keystroke syntax (`cmd-shift-,`).
    pub key: &'static str,
    pub scope: Scope,
    /// The action's name, for `+list-keybinds`.
    pub action: &'static str,
    /// What it does, in the user's words.
    pub label: &'static str,
    /// The menu item that shows this key, if any.
    pub menu: Option<&'static str>,
    /// When GPUI can't turn `key` into an NSMenu key equivalent (it passes
    /// "enter" through as the letter ⌘E), the key it should show instead,
    /// bound in a context that never matches.
    pub menu_key: Option<&'static str>,
    make: Option<fn(&str, Option<&str>) -> KeyBinding>,
    // --- buttons ---
    /// The toolbar icon (a name in `app::toolbar::ICONS`), when the row has a button.
    pub icon: Option<&'static str>,
    /// The toolbar button's short label (`show-buttons`); its tooltip is this
    /// plus the key (`tooltip`), so the buttons teach the keys.
    pub button: Option<&'static str>,
    /// Builds the action, so a button dispatches exactly what the key does.
    pub new_action: Option<fn() -> Box<dyn Action>>,
}

macro_rules! kb {
    ($key:expr, $scope:ident, $action:ident, $label:expr, $menu:expr) => {
        Keybind {
            key: $key,
            scope: Scope::$scope,
            action: stringify!($action),
            label: $label,
            menu: $menu,
            menu_key: None,
            make: Some(|k, c| KeyBinding::new(k, $action, c)),
            icon: None,
            button: None,
            new_action: Some(|| Box::new($action)),
        }
    };
}

/// A row with no key: it's in a menu (and maybe the toolbar) only.
macro_rules! menu_only {
    ($action:ident, $label:expr, $menu:expr) => {
        Keybind {
            key: "",
            scope: Scope::Global,
            action: stringify!($action),
            label: $label,
            menu: Some($menu),
            menu_key: None,
            make: None,
            icon: None,
            button: None,
            new_action: Some(|| Box::new($action)),
        }
    };
}

/// A toolbar button for a row: `button!(kb!(…), "icon", "Label")`.
macro_rules! button {
    ($row:expr, $icon:expr, $label:expr) => {
        Keybind {
            icon: Some($icon),
            button: Some($label),
            ..$row
        }
    };
}

/// The one table. Order matters only for display. Keys are written in
/// macOS terms; `platform_keys` respells them for Windows.
pub fn table() -> Vec<Keybind> {
    platform_keys(vec![
        kb!(
            "cmd-l",
            Main,
            FocusSearch,
            "Search / the omnibar",
            Some("Post › Find…")
        ),
        button!(
            kb!(
                "cmd-n",
                Main,
                NewDraft,
                "New draft",
                Some("Post › New Draft")
            ),
            "plus",
            "New"
        ),
        button!(
            Keybind {
                menu_key: Some("cmd-↩"),
                ..kb!(
                    "cmd-enter",
                    MainInput,
                    Publish,
                    "Publish…",
                    Some("Post › Publish…")
                )
            },
            "send",
            "Publish"
        ),
        kb!(
            "cmd-enter",
            Main,
            Publish,
            "Publish… (focus outside a text field)",
            None
        ),
        kb!(
            "cmd-t",
            Main,
            ToggleKind,
            "Fragment ⇄ thread",
            Some("Post › Fragment ⇄ Thread")
        ),
        kb!(
            "cmd-e",
            Main,
            TogglePreview,
            "Preview on/off, in place",
            Some("View › Preview")
        ),
        kb!(
            "cmd-o",
            Main,
            OpenPermalink,
            "Open the post on the web",
            Some("Post › Open on the Web")
        ),
        kb!(
            "cmd-=",
            Main,
            FontBigger,
            "Bigger text",
            Some("View › Bigger")
        ),
        kb!("cmd-+", Main, FontBigger, "Bigger text", None),
        kb!(
            "cmd--",
            Main,
            FontSmaller,
            "Smaller text",
            Some("View › Smaller")
        ),
        kb!(
            "cmd-0",
            Main,
            FontReset,
            "Actual size",
            Some("View › Actual Size")
        ),
        kb!(
            "cmd-,",
            Global,
            OpenSettings,
            "Settings",
            Some("Blygger › Settings…")
        ),
        kb!(
            "cmd-shift-,",
            Global,
            ReloadConfig,
            "Reload the config file",
            Some("Blygger › Reload Config")
        ),
        kb!(
            "cmd-<",
            Global,
            ReloadConfig,
            "Reload the config file",
            None
        ),
        kb!(
            "cmd-q",
            Global,
            Quit,
            "Quit",
            Some("Blygger › Quit Blygger")
        ),
        kb!(
            "ctrl-alt-cmd-o",
            Main,
            FakeToggleOffline,
            "Simulate offline (BLYGGER_FAKE only)",
            None
        ),
        kb!(
            "ctrl-alt-cmd-c",
            Main,
            FakeConflict,
            "Simulate a conflict (BLYGGER_FAKE only)",
            None
        ),
        // --- full editor ---
        button!(
            kb!(
                "cmd-1",
                Main,
                ViewWrite,
                "Write: list + editor",
                Some("View › Write")
            ),
            "panel-left",
            "Write"
        ),
        button!(
            kb!(
                "cmd-2",
                Main,
                ViewSplit,
                "List + editor + preview",
                Some("View › List + Editor + Preview")
            ),
            "columns-3",
            "Preview"
        ),
        button!(
            kb!(
                "cmd-3",
                Main,
                ViewStudio,
                "Full editor: editor + preview",
                Some("View › Full Editor")
            ),
            "columns-2",
            "Full editor"
        ),
        // --- end full editor ---
        // Menu-only (no key): listed so the table is the whole story.
        menu_only!(
            Disconnect,
            "Disconnect from this blyg",
            "Blygger › Disconnect…"
        ),
        menu_only!(
            OpenConfigFile,
            "Open the config file",
            "Blygger › Open Config File"
        ),
        // --- auto-update ---
        menu_only!(
            CheckForUpdates,
            "Check for a new release now (whatever auto-update says)",
            "Blygger › Check for Updates…"
        ),
        // --- buttons --- The quick-capture hotkey is `capture-hotkey`, not a
        // binding here; its button's tooltip shows the configured one.
        button!(
            menu_only!(
                ShowCapture,
                "Quick capture (the global hotkey, capture-hotkey)",
                "Blygger › Quick Capture"
            ),
            "zap",
            "Capture"
        ),
        // --- end buttons ---
        // --- scratch notes ---
        // Global so the quick-capture panel (no key context) gets them too;
        // gpui-base's inputs bind neither key.
        button!(
            kb!(
                "cmd-d",
                Global,
                MakeDraft,
                "Make draft (a scratch note → a draft on the blyg)",
                Some("Post › Make Draft")
            ),
            "file-up",
            "Make draft"
        ),
        button!(
            kb!(
                "cmd-s",
                Global,
                KeepCapture,
                "Quick capture: keep it (scratch, or a draft per capture-default)",
                None
            ),
            "sticky-note",
            "Scratch"
        ),
        // --- end scratch notes ---
        // --- AI ---
        button!(
            kb!(
                "cmd-g",
                Main,
                AiGenerate,
                "Generate: fill the [TK] under the caret, or open the AI helpers",
                Some("Post › Generate (TK)")
            ),
            "sparkles",
            "Generate"
        ),
        kb!(
            "cmd-shift-g",
            Main,
            AiShorten,
            "Shorten to fit 1000 (AI; you accept or reject)",
            Some("Post › Shorten to Fit 1000")
        ),
        // --- end AI ---
        // --- reading & versions ---
        kb!(
            "cmd-r",
            Main,
            ShowReading,
            "Reading list (again: back to posts)",
            Some("Blyg › Reading")
        ),
        kb!(
            "cmd-shift-m",
            Main,
            ShowMentions,
            "Mentions & responses",
            Some("Blyg › Mentions")
        ),
        kb!(
            "cmd-shift-s",
            Main,
            ShowSubscriptions,
            "Subscriptions",
            Some("Blyg › Subscriptions")
        ),
        button!(
            kb!(
                "cmd-y",
                Main,
                ShowVersions,
                "Versions of this post",
                Some("Post › Versions…")
            ),
            "rotate-ccw-clock",
            "Versions"
        ),
        kb!(
            "cmd-k",
            Main,
            QuotePicker,
            "Quote a post in this thread",
            Some("Post › Quote…")
        ),
        menu_only!(
            SubscribeTo,
            "Subscribe to a blyg or feed",
            "Blyg › Subscribe…"
        ),
        menu_only!(
            SiteSettings,
            "The blyg's site settings (title, bio, links)",
            "Blyg › Site Settings…"
        ),
        // --- end reading & versions ---
        // --- delete & withdraw --- (docs/SPEC.md rule 5: published work is
        // withdrawn, never deleted). ⇧⌘⌫, not ⌘⌫: that's delete-to-line-start
        // in every text field. Withdraw is irreversible, so it has no key.
        button!(
            kb!(
                "cmd-shift-backspace",
                Main,
                DeleteDraft,
                "Delete this draft or scratch note (asks first)",
                Some("Post › Delete Draft…")
            ),
            "trash-2",
            "Delete"
        ),
        button!(
            menu_only!(
                Withdraw,
                "Withdraw this published post (asks first; permanent and visible)",
                "Post › Withdraw…"
            ),
            "archive-x",
            "Withdraw"
        ),
        // --- end delete & withdraw ---
        // --- profiles --- (inside the sheet: esc, ↑/↓, ⏎, F, ⇥ and ← are
        // the sheet's own keys, like the reading screens' ↑/↓)
        kb!(
            "cmd-i",
            Main,
            ShowProfile,
            "Profile of this post's author (yours on your own post); again closes it",
            Some("Blyg › Profile")
        ),
        kb!(
            "cmd-shift-o",
            Main,
            OpenProfile,
            "Open a profile: paste a blyg, a post or a feed",
            Some("Blyg › Open Profile…")
        ),
        menu_only!(
            MyProfile,
            "Your own profile, as visitors see it",
            "Blyg › My Profile"
        ),
        // --- end profiles ---
        // --- onboarding ---
        kb!(
            "alt-cmd-right",
            Main,
            TutorialNext,
            "Tutorial: next step",
            None
        ),
        kb!(
            "alt-cmd-left",
            Main,
            TutorialBack,
            "Tutorial: previous step",
            None
        ),
        menu_only!(
            ShowTutorial,
            "The interactive tutorial (on sample data)",
            "Help › Blygger Tutorial"
        ),
        // --- end onboarding ---
    ])
}

/// The table as this platform spells it. On Windows GPUI's `cmd` is the
/// Windows key, which the system owns, so every `cmd` becomes `ctrl`.
fn platform_keys(rows: Vec<Keybind>) -> Vec<Keybind> {
    #[cfg(target_os = "windows")]
    {
        let mut rows = rows;
        for k in &mut rows {
            k.key = platform_key(k.key);
            k.menu_key = k.menu_key.map(platform_key);
        }
        rows
    }
    #[cfg(not(target_os = "windows"))]
    rows
}

/// `cmd-shift-,` → `ctrl-shift-,` (and `ctrl-alt-cmd-o` → `ctrl-alt-o`).
/// Interned, so the table can keep `&'static str` keys.
#[cfg(target_os = "windows")]
pub fn platform_key(key: &'static str) -> &'static str {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static SPELLED: OnceLock<Mutex<HashMap<&'static str, &'static str>>> = OnceLock::new();
    if !key.split('-').any(|p| p == "cmd") {
        return key;
    }
    let mut spelled = SPELLED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    spelled
        .entry(key)
        .or_insert_with(|| Box::leak(respell_for_windows(key).into_boxed_str()))
}

/// Keys whose plain respelling would take a Windows convention: Ctrl+Y is
/// Redo in every Windows text field (and gpui-base's `Input` binds it).
const WINDOWS_OVERRIDES: &[(&str, &str)] = &[("cmd-y", "ctrl-shift-y")];

/// The pure part of `platform_key`.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub fn respell_for_windows(key: &str) -> String {
    if let Some((_, win)) = WINDOWS_OVERRIDES.iter().find(|(mac, _)| *mac == key) {
        return win.to_string();
    }
    let (body, minus) = match key.strip_suffix("--") {
        Some(b) => (b, true),
        None => (key, false),
    };
    let mut out: Vec<&str> = Vec::new();
    for p in body.split('-') {
        let p = if p == "cmd" { "ctrl" } else { p };
        if !(matches!(p, "ctrl" | "alt" | "shift") && out.contains(&p)) {
            out.push(p);
        }
    }
    let mut s = out.join("-");
    if minus {
        s.push_str("--");
    }
    s
}

/// Key hints written into the app's text (`"then ⌘G again"`), as this
/// platform spells them: unchanged on macOS; on Windows every run of
/// modifier glyphs and its key is respelled like the bindings
/// (`⇧⌘G` → `Ctrl+Shift+G`, `⌘Y` → `Ctrl+Shift+Y`). Interned, so it can
/// stand wherever the literal did.
pub fn hint(text: &'static str) -> &'static str {
    #[cfg(target_os = "windows")]
    {
        use std::collections::HashMap;
        use std::sync::{Mutex, OnceLock};
        static SPELLED: OnceLock<Mutex<HashMap<&'static str, &'static str>>> = OnceLock::new();
        let mut spelled = SPELLED
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        spelled
            .entry(text)
            .or_insert_with(|| Box::leak(windows_hint(text).into_boxed_str()))
    }
    #[cfg(not(target_os = "windows"))]
    text
}

/// `hint` for text built at runtime (`format!`).
pub fn hint_owned(text: String) -> String {
    if cfg!(target_os = "windows") {
        windows_hint(&text)
    } else {
        text
    }
}

/// The pure part of `hint`: rewrite each run of macOS modifier glyphs
/// (`⌃⌥⇧⌘`) and the key after it as Windows writes the respelled binding.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub fn windows_hint(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let mut mods = Vec::new();
        while i < chars.len() {
            match chars[i] {
                '⌃' => mods.push("ctrl"),
                '⌥' => mods.push("alt"),
                '⇧' => mods.push("shift"),
                '⌘' => mods.push("cmd"),
                _ => break,
            }
            i += 1;
        }
        if mods.is_empty() {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        // The key: a named key, one glyph, or one character; none when the
        // modifiers stand alone ("⌘-click", "hold ⌘").
        let rest: String = chars[i..].iter().collect();
        let (key, used) = if rest.starts_with("Space") {
            (Some("space".to_string()), 5)
        } else if rest.starts_with("esc") {
            (Some("escape".to_string()), 3)
        } else {
            match chars.get(i) {
                Some('⏎') => (Some("enter".into()), 1),
                Some('⌫') => (Some("backspace".into()), 1),
                Some('⇥') => (Some("tab".into()), 1),
                Some('←') => (Some("left".into()), 1),
                Some('→') => (Some("right".into()), 1),
                Some('↑') => (Some("up".into()), 1),
                Some('↓') => (Some("down".into()), 1),
                Some('-') if chars.get(i + 1).is_some_and(|c| c.is_alphabetic()) => (None, 0),
                Some(c) if !c.is_whitespace() && !matches!(c, ')' | '·' | '/') => {
                    (Some(c.to_lowercase().collect()), 1)
                }
                _ => (None, 0),
            }
        };
        let Some(key) = key else {
            // Modifiers alone: name them the Windows way.
            let names: Vec<&str> = mods
                .iter()
                .map(|m| match *m {
                    "cmd" | "ctrl" => "Ctrl",
                    "alt" => "Alt",
                    _ => "Shift",
                })
                .collect();
            out.push_str(&names.join("+"));
            continue;
        };
        // `cmd--` is how the table spells a minus key.
        let mac = format!("{}-{}", mods.join("-"), key);
        out.push_str(&windows_label(&respell_for_windows(&mac)));
        i += used;
    }
    windows_words(&out)
}

/// Mac words in the app's text, as Windows says them, longest first so a
/// phrase wins over the word inside it.
const WINDOWS_WORDS: &[(&str, &str)] = &[
    ("your macOS Keychain", "Windows Credential Manager"),
    ("the macOS Keychain", "Windows Credential Manager"),
    ("macOS Keychain", "Windows Credential Manager"),
    ("your Keychain", "Credential Manager"),
    ("the Keychain", "Credential Manager"),
    ("Keychain", "Credential Manager"),
    ("follow macOS", "follow Windows"),
    ("this Mac", "this PC"),
    ("your Mac", "your PC"),
    ("another Mac", "another PC"),
    // Windows has no menu bar; the Menu button lists the same items.
    ("the Blygger menu", "the Menu button"),
];

/// The wording half of `windows_hint`: Keychain → Credential Manager,
/// this Mac → this PC.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub fn windows_words(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    'scan: while !rest.is_empty() {
        for (mac, win) in WINDOWS_WORDS {
            if let Some(after) = rest.strip_prefix(mac) {
                out.push_str(win);
                rest = after;
                continue 'scan;
            }
        }
        let c = rest.chars().next().expect("not empty");
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// Tests write keystrokes in macOS terms, like the table. On Windows they
/// are respelled exactly as the bindings are (`respell_for_windows`).
#[cfg(test)]
pub fn keys(keystrokes: &str) -> String {
    if cfg!(target_os = "windows") {
        keystrokes
            .split(' ')
            .map(respell_for_windows)
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        keystrokes.to_string()
    }
}

/// Register every binding. Call after `gpui_kit::init`, so that ours are
/// added later and win ties at the same depth.
pub fn bind_keys(cx: &mut App) {
    let table = table();
    let mut out = Vec::new();
    // Menu-equivalent shims first: the native menu uses the *first* binding
    // of an action when none matches its default context.
    for k in &table {
        if let (Some(mk), Some(make)) = (k.menu_key, k.make) {
            out.push(make(mk, Some(MENU_ONLY)));
        }
    }
    for k in &table {
        if let Some(make) = k.make {
            out.push(make(k.key, k.scope.context()));
        }
    }
    cx.bind_keys(out);
    // Menu-only actions have no keys; keep the compiler honest that they exist.
    let _ = (Disconnect, OpenConfigFile, SiteSettings, SubscribeTo);
    let _ = ShowTutorial; // --- onboarding ---
    let _ = MyProfile; // --- profiles ---
    let _ = Withdraw; // --- delete & withdraw ---
}

/// `Ctrl+Shift+,` for `ctrl-shift-,` (Windows spells keys out).
#[cfg(target_os = "windows")]
pub fn glyphs(key: &str) -> String {
    if key.is_empty() {
        return "—".into();
    }
    windows_label(key)
}

/// A key as Windows menus and tooltips write it: `Ctrl+Shift+Enter`.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub fn windows_label(key: &str) -> String {
    let (mods, last) = normalize(key);
    let mut parts: Vec<String> = [
        ("ctrl", "Ctrl"),
        ("alt", "Alt"),
        ("shift", "Shift"),
        ("cmd", "Win"),
    ]
    .iter()
    .filter(|(m, _)| mods.contains(m))
    .map(|(_, name)| name.to_string())
    .collect();
    parts.push(match last.as_str() {
        "enter" => "Enter".into(),
        "space" => "Space".into(),
        "escape" => "Esc".into(),
        "tab" => "Tab".into(),
        "backspace" => "Backspace".into(),
        "up" | "down" | "left" | "right" => {
            let mut c = last.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect())
                .unwrap_or_default()
        }
        k => k.to_uppercase(),
    });
    parts.join("+")
}

/// `⌘⇧,` for `cmd-shift-,`.
#[cfg(not(target_os = "windows"))]
pub fn glyphs(key: &str) -> String {
    if key.is_empty() {
        return "—".into();
    }
    let mut mods = String::new();
    let mut last = "";
    let parts: Vec<&str> = match key.strip_suffix("--") {
        Some(stripped) => {
            let mut p: Vec<&str> = stripped.split('-').collect();
            p.push("-");
            p
        }
        None => key.split('-').collect(),
    };
    for p in &parts {
        match *p {
            "ctrl" => mods.push('⌃'),
            "alt" => mods.push('⌥'),
            "shift" => mods.push('⇧'),
            "cmd" => mods.push('⌘'),
            k => last = k,
        }
    }
    // macOS order: ⌃⌥⇧⌘
    let order = ['⌃', '⌥', '⇧', '⌘'];
    let mut sorted: String = order.iter().filter(|c| mods.contains(**c)).collect();
    let k = match last {
        "enter" => "⏎".to_string(),
        "space" => "Space".to_string(),
        "escape" => "esc".to_string(),
        "tab" => "⇥".to_string(),
        "backspace" => "⌫".to_string(),
        k => k.to_uppercase(),
    };
    sorted.push_str(&k);
    sorted
}

// --- buttons ---

/// The title-bar toolbar (`show-buttons`): groups of actions, left to right.
/// Each must be a table row with a `button`.
pub const TOOLBAR: &[&[&str]] = &[
    &["NewDraft", "MakeDraft", "Publish"],
    &["ViewWrite", "ViewSplit", "ViewStudio"],
    &["ShowVersions", "AiGenerate"],
    // One slot: Delete for a draft or scratch note, Withdraw for a published
    // post (`toolbar::visible`).
    &["DeleteDraft", "Withdraw"],
    &["ShowCapture"],
];

/// Quick capture's row (`show-buttons`): the action and the panel's own,
/// shorter, label. Each must be a table row with a key.
pub const CAPTURE_ROW: &[(&str, &str)] = &[
    ("KeepCapture", "Scratch"),
    ("MakeDraft", "Draft"),
    ("Publish", "Publish"),
];

/// The row a button comes from: the first row of `action` that has a
/// button (or, failing that, a key).
pub fn button_row(action: &str) -> Option<Keybind> {
    let rows: Vec<Keybind> = table().into_iter().filter(|k| k.action == action).collect();
    let pick = rows
        .iter()
        .position(|k| k.button.is_some())
        .or_else(|| rows.iter().position(|k| !k.key.is_empty()))?;
    rows.into_iter().nth(pick)
}

/// The key a button teaches: the row's key, or for quick capture the
/// configured `capture-hotkey`.
pub fn shortcut(k: &Keybind, capture_hotkey: &str) -> String {
    match (k.key, k.action) {
        ("", "ShowCapture") => crate::prefs::hotkey_glyphs(capture_hotkey),
        ("", _) => String::new(),
        (key, _) => glyphs(key),
    }
}

/// A button's tooltip: "Publish  ⌘⏎".
pub fn tooltip(label: &str, k: &Keybind, capture_hotkey: &str) -> String {
    match shortcut(k, capture_hotkey) {
        s if s.is_empty() => label.to_string(),
        s => format!("{label}  {s}"),
    }
}
// --- end buttons ---

/// Normalise a key in either our syntax (`cmd-shift-g`) or the capture
/// hotkey's (`ctrl+alt+B`) to a comparable (modifiers, key) pair.
pub fn normalize(key: &str) -> (Vec<&'static str>, String) {
    let key = key.trim();
    let parts: Vec<String> = if let Some(stripped) = key.strip_suffix("--") {
        let mut p: Vec<String> = stripped.split(['-', '+']).map(str::to_string).collect();
        p.push("-".into());
        p
    } else {
        key.split(['-', '+']).map(str::to_string).collect()
    };
    let mut mods = Vec::new();
    let mut last = String::new();
    for p in parts {
        match p.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods.push("ctrl"),
            "alt" | "option" | "opt" => mods.push("alt"),
            "shift" => mods.push("shift"),
            "cmd" | "command" | "super" | "meta" => mods.push("cmd"),
            k => {
                // global-hotkey spells letters `KeyB`, digits `Digit1`.
                let k = k
                    .strip_prefix("key")
                    .filter(|r| r.len() == 1)
                    .or_else(|| k.strip_prefix("digit").filter(|r| r.len() == 1))
                    .unwrap_or(k);
                last = match k {
                    "return" => "enter".into(),
                    "esc" => "escape".into(),
                    k => k.to_string(),
                };
            }
        }
    }
    mods.sort_unstable();
    mods.dedup();
    (mods, last)
}

/// The app binding (if any) the capture hotkey `hotkey` would shadow.
pub fn clash_with_hotkey(hotkey: &str) -> Option<Keybind> {
    let want = normalize(hotkey);
    table()
        .into_iter()
        .find(|k| !k.key.is_empty() && normalize(k.key) == want)
}

/// Windows shortcuts the system (or every Windows app) owns, in the
/// respelled form `table()` produces there.
#[cfg(all(test, target_os = "windows"))]
pub const RESERVED: &[(&str, &str, Option<&str>)] = &[
    ("ctrl-q", "Quit", Some("Quit")),
    ("alt-f4", "Close window", None),
    ("alt-tab", "App switcher", None),
    ("alt-shift-tab", "App switcher", None),
    ("ctrl-escape", "Start menu", None),
    ("ctrl-shift-escape", "Task Manager", None),
    ("ctrl-alt-delete", "Security screen", None),
    ("alt-space", "Window menu", None),
    ("ctrl-c", "Copy", None),
    ("ctrl-v", "Paste", None),
    ("ctrl-x", "Cut", None),
    ("ctrl-z", "Undo", None),
    ("ctrl-y", "Redo", None),
    ("ctrl-shift-z", "Redo", None),
    ("ctrl-a", "Select all", None),
];

/// macOS shortcuts the system (or every Mac app) owns. An entry's action is
/// the only one of ours allowed on that key (⌘Q is our Quit).
#[cfg(all(test, not(target_os = "windows")))]
pub const RESERVED: &[(&str, &str, Option<&str>)] = &[
    ("cmd-q", "Quit", Some("Quit")),
    ("cmd-w", "Close window", None),
    ("cmd-h", "Hide app", None),
    ("cmd-alt-h", "Hide others", None),
    ("cmd-m", "Minimize", None),
    ("cmd-tab", "App switcher", None),
    ("cmd-shift-tab", "App switcher", None),
    ("cmd-`", "Cycle windows", None),
    ("cmd-space", "Spotlight", None),
    ("ctrl-cmd-space", "Emoji & symbols", None),
    ("cmd-alt-escape", "Force quit", None),
    ("ctrl-cmd-q", "Lock screen", None),
    ("ctrl-cmd-f", "Full screen", None),
    ("cmd-shift-3", "Screenshot", None),
    ("cmd-shift-4", "Screenshot", None),
    ("cmd-shift-5", "Screenshot", None),
    ("cmd-alt-d", "Dock", None),
    ("cmd-shift-/", "Help menu", None),
    ("cmd-c", "Copy", None),
    ("cmd-v", "Paste", None),
    ("cmd-x", "Cut", None),
    ("cmd-z", "Undo", None),
    ("cmd-shift-z", "Redo", None),
    ("cmd-a", "Select all", None),
];

/// Toolbar buttons with no key on purpose: Withdraw is permanent, so it's
/// never one keystroke away (docs/SPEC.md § Buttons).
#[cfg(test)]
pub const KEYLESS_BUTTONS: &[&str] = &["Withdraw"];

/// Named keys the native menu does turn into their key equivalent
/// (gpui-pre-macos `key_to_native`), unlike "enter".
#[cfg(all(test, not(target_os = "windows")))]
pub const NATIVE_MENU_KEYS: &[&str] = &["backspace"];

/// Keys we bind on purpose over one of gpui-base's own `Input` bindings.
#[cfg(all(test, not(target_os = "windows")))]
pub const OUTRANKS_INPUT: &[&str] = &["cmd-enter"];
#[cfg(all(test, target_os = "windows"))]
pub const OUTRANKS_INPUT: &[&str] = &["ctrl-enter"];

/// `blygger +list-keybinds`.
pub fn list() -> String {
    let mut out = String::from("# key        action              where        what\n");
    for k in table() {
        if k.key.is_empty() && k.menu.is_none() {
            continue;
        }
        let place = match k.scope {
            Scope::Global => "anywhere",
            Scope::Main => "main window",
            Scope::MainInput => "text fields",
        };
        let mut line = format!(
            "{:<12} {:<19} {:<12} {}",
            glyphs(k.key),
            k.action,
            place,
            k.label
        );
        if let Some(m) = k.menu {
            line.push_str(&format!("  [{m}]"));
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn live() -> Vec<Keybind> {
        table().into_iter().filter(|k| !k.key.is_empty()).collect()
    }

    #[test]
    fn no_duplicate_keys_in_one_place() {
        let mut seen: HashMap<(&str, (Vec<&str>, String)), &str> = HashMap::new();
        for k in live() {
            // Main and MainInput are the same place; the MainInput row is the
            // deliberate twin of a Main row with the same action.
            let slot = (k.scope.place(), normalize(k.key));
            if let Some(prev) = seen.get(&slot) {
                assert_eq!(
                    *prev, k.action,
                    "{} is bound to both {prev} and {}",
                    k.key, k.action
                );
            }
            seen.insert(slot, k.action);
        }
    }

    #[test]
    fn nothing_shadows_or_is_shadowed_by_gpui_base_inputs() {
        // gpui-base's Input bindings, as registered by gpui_kit::init.
        let input_keys = {
            let cx = gpui_kit::TestAppContext::single();
            cx.update(|cx| {
                gpui_kit::init(cx);
                let km = cx.key_bindings();
                let km = km.borrow();
                km.bindings()
                    .filter(|b| {
                        b.predicate()
                            .is_some_and(|p| format!("{p}").contains("Input"))
                    })
                    .filter_map(|b| {
                        let ks = b.keystrokes();
                        (ks.len() == 1).then(|| ks[0].unparse())
                    })
                    .map(|s| normalize(&s))
                    .collect::<Vec<_>>()
            })
        };
        assert!(!input_keys.is_empty(), "read gpui-base's bindings");
        for k in live().iter().filter(|k| k.scope != Scope::Global) {
            let shadowed = input_keys.contains(&normalize(k.key));
            let deliberate = OUTRANKS_INPUT.contains(&k.key);
            assert!(
                !shadowed || deliberate,
                "{} ({}) is also a gpui-base Input key: it would stop working while a text \
                 field has focus. Pick another key, or out-rank Input and list it in OUTRANKS_INPUT",
                k.key,
                k.action
            );
            if deliberate {
                // Out-ranking needs a MainInput binding for the same key.
                assert!(
                    live()
                        .iter()
                        .any(|o| o.key == k.key && o.scope == Scope::MainInput),
                    "{} needs a MainInput binding to out-rank Input",
                    k.key
                );
            }
        }
    }

    #[test]
    fn capture_hotkey_is_free() {
        assert!(
            clash_with_hotkey(crate::prefs::DEFAULT_HOTKEY).is_none(),
            "the default quick-capture hotkey clashes with an app binding"
        );
        // Windows respells ⌘ as Ctrl (`platform_keys`).
        let cmd = if cfg!(target_os = "windows") {
            "ctrl"
        } else {
            "cmd"
        };
        assert_eq!(
            clash_with_hotkey(&format!("{cmd}+T")).map(|k| k.action),
            Some("ToggleKind")
        );
        assert_eq!(
            clash_with_hotkey(&format!("{}+KeyL", cmd.to_uppercase())).map(|k| k.action),
            Some("FocusSearch")
        );
    }

    #[test]
    fn no_reserved_macos_shortcuts() {
        for k in live() {
            if let Some((_, what, allowed)) = RESERVED
                .iter()
                .find(|(r, _, _)| normalize(r) == normalize(k.key))
            {
                assert_eq!(
                    Some(k.action),
                    *allowed,
                    "{} is macOS's {what}; don't bind {} to it",
                    k.key,
                    k.action
                );
            }
        }
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn planned_keys_are_reserved() {
        assert!(table().iter().any(|k| k.key == "cmd-l"));
        // ⌘Y graduated from planned to the version browser.
        assert!(
            table()
                .iter()
                .any(|k| k.key == "cmd-y" && k.action == "ShowVersions")
        );
        // ⌘. was pressed by a tester expecting something; it stays unbound.
        assert!(!table().iter().any(|k| k.key == "cmd-."));
    }

    #[test]
    fn keys_respell_for_windows() {
        assert_eq!(respell_for_windows("cmd-l"), "ctrl-l");
        assert_eq!(respell_for_windows("cmd-shift-,"), "ctrl-shift-,");
        assert_eq!(respell_for_windows("ctrl-alt-cmd-o"), "ctrl-alt-o");
        assert_eq!(respell_for_windows("cmd--"), "ctrl--");
        assert_eq!(respell_for_windows("cmd-shift--"), "ctrl-shift--");
        assert_eq!(respell_for_windows("alt-up"), "alt-up");
        // Ctrl+Y is Redo on Windows, so Versions moves over.
        assert_eq!(respell_for_windows("cmd-y"), "ctrl-shift-y");
        assert_eq!(windows_label("ctrl-shift-,"), "Ctrl+Shift+,");
        assert_eq!(windows_label("ctrl-enter"), "Ctrl+Enter");
        assert_eq!(windows_label("ctrl--"), "Ctrl+-");
        assert_eq!(windows_label("ctrl+alt+b"), "Ctrl+Alt+B");
        assert_eq!(windows_label("alt-up"), "Alt+Up");
    }

    #[test]
    fn hints_respell_for_windows() {
        assert_eq!(windows_hint("then ⌘G again"), "then Ctrl+G again");
        assert_eq!(windows_hint("⇧⌘G"), "Ctrl+Shift+G");
        assert_eq!(windows_hint("⌘⏎ publishes"), "Ctrl+Enter publishes");
        assert_eq!(windows_hint("Settings (⌘,)"), "Settings (Ctrl+,)");
        assert_eq!(windows_hint("⌘Y versions"), "Ctrl+Shift+Y versions");
        assert_eq!(windows_hint("⌃⌥B"), "Ctrl+Alt+B");
        assert_eq!(windows_hint("⇧⌘Space"), "Ctrl+Shift+Space");
        assert_eq!(windows_hint("⌘-click"), "Ctrl-click");
        assert_eq!(windows_hint("hold ⌘ and"), "hold Ctrl and");
        assert_eq!(windows_hint("⌘1 · ⌘2"), "Ctrl+1 · Ctrl+2");
        assert_eq!(windows_hint("no keys here ⏎"), "no keys here ⏎");
        assert_eq!(
            windows_hint("The token is kept in your macOS Keychain; the address"),
            "The token is kept in Windows Credential Manager; the address"
        );
        assert_eq!(
            windows_hint("saved in your Keychain"),
            "saved in Credential Manager"
        );
        assert_eq!(
            windows_hint("scratch · only on this Mac"),
            "scratch · only on this PC"
        );
        assert_eq!(windows_hint("Macintosh"), "Macintosh");
        // macOS text is untouched.
        if !cfg!(target_os = "windows") {
            assert_eq!(hint("then ⌘G"), "then ⌘G");
        }
    }

    /// On Windows every key is respelled, `⌘` never survives, and the
    /// table still has no clashes (`no_duplicate_keys_in_one_place`).
    #[test]
    #[cfg(target_os = "windows")]
    fn windows_keys_use_ctrl() {
        assert!(live().iter().all(|k| !k.key.split('-').any(|p| p == "cmd")));
        assert!(table().iter().any(|k| k.key == "ctrl-l"));
        assert!(
            table()
                .iter()
                .any(|k| k.key == "ctrl-shift-y" && k.action == "ShowVersions")
        );
        assert_eq!(glyphs("ctrl-enter"), "Ctrl+Enter");
        assert!(list().contains("Publish"));
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn glyphs_and_listing() {
        assert_eq!(glyphs("cmd-shift-,"), "⇧⌘,");
        assert_eq!(glyphs("cmd-enter"), "⌘⏎");
        assert_eq!(glyphs("ctrl-alt-cmd-o"), "⌃⌥⌘O");
        assert_eq!(glyphs("cmd--"), "⌘-");
        assert_eq!(glyphs("cmd-shift-backspace"), "⇧⌘⌫");
        let l = list();
        assert!(l.contains("⌘⏎") && l.contains("Publish"), "{l}");
        assert_eq!(
            normalize("ctrl+alt+B"),
            (vec!["alt", "ctrl"], "b".to_string())
        );
        assert_eq!(normalize("cmd--"), (vec!["cmd"], "-".to_string()));
    }

    // --- buttons ---

    /// Buttons come from the table: every toolbar / capture-row action is a
    /// row with a button (or key), its action is really bound to its key, a
    /// click builds the same action, and the tooltip teaches the key.
    #[test]
    fn every_button_is_a_bound_action_with_its_key_in_the_tooltip() {
        let cx = gpui_kit::TestAppContext::single();
        cx.update(|cx| {
            gpui_kit::init(cx);
            bind_keys(cx);
            let km = cx.key_bindings();
            let km = km.borrow();
            let toolbar = TOOLBAR.iter().flat_map(|g| g.iter().copied());
            let capture = CAPTURE_ROW.iter().map(|(a, _)| *a);
            let mut seen = 0;
            for action in toolbar.chain(capture) {
                let k = button_row(action).unwrap_or_else(|| panic!("{action}: no row"));
                seen += 1;
                let make = k
                    .new_action
                    .unwrap_or_else(|| panic!("{action}: no action"));
                let built = make();
                assert_eq!(
                    built.name().rsplit("::").next(),
                    Some(action),
                    "{action}'s button builds {}",
                    built.name()
                );
                if TOOLBAR.iter().any(|g| g.contains(&action)) {
                    assert!(k.button.is_some() && k.icon.is_some(), "{action}");
                    assert!(
                        crate::app::toolbar::icon_svg(k.icon.unwrap()).is_some(),
                        "{action}: icon {:?} isn't bundled",
                        k.icon
                    );
                }
                let tip = tooltip(k.button.unwrap_or(action), &k, "ctrl+alt+b");
                if k.key.is_empty() {
                    if action == "ShowCapture" {
                        // Quick capture: the configured global hotkey.
                        let want = if cfg!(target_os = "windows") {
                            "  Ctrl+Alt+B"
                        } else {
                            "  ⌃⌥B"
                        };
                        assert!(tip.ends_with(want), "{tip}");
                    } else {
                        // Keyless on purpose (Withdraw is irreversible): menu only.
                        assert_eq!(KEYLESS_BUTTONS, &[action], "{action} has no key");
                        assert!(k.menu.is_some(), "{action}: a keyless button needs a menu");
                        assert_eq!(tip, k.button.unwrap(), "{tip}");
                    }
                    continue;
                }
                assert!(tip.contains(&glyphs(k.key)), "{tip} lacks {}", k.key);
                // Really bound: the key, in the keymap, runs this action.
                let bound = km.bindings_for_action(&*built).any(|b| {
                    let ks = b.keystrokes();
                    ks.len() == 1 && normalize(&ks[0].unparse()) == normalize(k.key)
                });
                assert!(
                    bound,
                    "{action}'s button teaches {}, which isn't bound to it",
                    k.key
                );
            }
            assert_eq!(seen, 14);
            // Only listed buttons exist (no orphan labels in the table).
            for k in table().iter().filter(|k| k.button.is_some()) {
                assert!(
                    TOOLBAR.iter().any(|g| g.contains(&k.action))
                        || CAPTURE_ROW.iter().any(|(a, _)| *a == k.action),
                    "{} has a button but no place",
                    k.action
                );
            }
        });
        assert_eq!(
            tooltip("Publish", &button_row("Publish").unwrap(), ""),
            if cfg!(target_os = "windows") {
                "Publish  Ctrl+Enter"
            } else {
                "Publish  ⌘⏎"
            }
        );
    }

    /// The native menu shows the first binding of an action whose context
    /// isn't its default one (gpui-pre-macos `create_menu_item`), and turns
    /// its key into a key equivalent verbatim: "enter" became the letter e
    /// (⌘E on Publish). The first Publish binding must be the ⌘↩ shim.
    #[test]
    #[cfg(not(target_os = "windows"))]
    fn the_publish_menu_shows_cmd_return() {
        let cx = gpui_kit::TestAppContext::single();
        cx.update(|cx| {
            gpui_kit::init(cx);
            bind_keys(cx);
            let km = cx.key_bindings();
            let km = km.borrow();
            let first = km.bindings_for_action(&Publish).next().expect("bound");
            let ks = first.keystrokes();
            assert_eq!(ks.len(), 1);
            assert_eq!(ks[0].key(), "↩");
            assert!(ks[0].modifiers().platform);
            // And no binding of any action puts a multi-letter key first
            // where the menu would read only its first letter.
            for k in table()
                .iter()
                .filter(|k| k.menu.is_some() && !k.key.is_empty())
            {
                let key = k.menu_key.unwrap_or(k.key);
                let last = normalize(key).1;
                assert!(
                    last.chars().count() == 1
                        || k.menu_key.is_some()
                        || NATIVE_MENU_KEYS.contains(&last.as_str()),
                    "{} ({}) would show as ⌘{} in the menu; give it a menu_key",
                    k.key,
                    k.action,
                    last.chars().next().unwrap_or(' ').to_uppercase()
                );
            }
        });
    }
}
