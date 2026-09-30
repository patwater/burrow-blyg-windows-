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

/// The one table. Order matters only for display.
pub fn table() -> Vec<Keybind> {
    vec![
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
    ]
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

/// `⌘⇧,` for `cmd-shift-,`.
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

/// macOS shortcuts the system (or every Mac app) owns. An entry's action is
/// the only one of ours allowed on that key (⌘Q is our Quit).
#[cfg(test)]
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
#[cfg(test)]
pub const NATIVE_MENU_KEYS: &[&str] = &["backspace"];

/// Keys we bind on purpose over one of gpui-base's own `Input` bindings.
#[cfg(test)]
pub const OUTRANKS_INPUT: &[&str] = &["cmd-enter"];

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
        assert_eq!(
            clash_with_hotkey("cmd+T").map(|k| k.action),
            Some("ToggleKind")
        );
        assert_eq!(
            clash_with_hotkey("CMD+KeyL").map(|k| k.action),
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
                        assert!(tip.ends_with("  ⌃⌥B"), "{tip}");
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
            "Publish  ⌘⏎"
        );
    }

    /// The native menu shows the first binding of an action whose context
    /// isn't its default one (gpui-pre-macos `create_menu_item`), and turns
    /// its key into a key equivalent verbatim: "enter" became the letter e
    /// (⌘E on Publish). The first Publish binding must be the ⌘↩ shim.
    #[test]
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
