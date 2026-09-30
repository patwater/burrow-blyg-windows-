//! The UI's view of the config file: fonts, size, theme, layout and the
//! capture hotkey. Built from `blyg_core::Config`; changes are written back
//! to the config file as just the keys that changed (see `crate::settings`).

use blyg_core::config::{CaptureDefault, Change, Config, Layout, NewNote, Theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemePref {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemePref {
    pub const ALL: [ThemePref; 3] = [ThemePref::System, ThemePref::Light, ThemePref::Dark];
    pub fn label(self) -> &'static str {
        match self {
            ThemePref::System => "System",
            ThemePref::Light => "Light",
            ThemePref::Dark => "Dark",
        }
    }
    /// The config value (`theme = …`).
    pub fn value(self) -> &'static str {
        match self {
            ThemePref::System => "system",
            ThemePref::Light => "light",
            ThemePref::Dark => "dark",
        }
    }
}

impl From<Theme> for ThemePref {
    fn from(t: Theme) -> Self {
        match t {
            Theme::System => ThemePref::System,
            Theme::Light => ThemePref::Light,
            Theme::Dark => ThemePref::Dark,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayoutPref {
    /// List to the left of the editor.
    #[default]
    Side,
    /// List above the editor.
    Stacked,
}

impl LayoutPref {
    pub const ALL: [LayoutPref; 2] = [LayoutPref::Side, LayoutPref::Stacked];
    pub fn label(self) -> &'static str {
        match self {
            LayoutPref::Side => "Side by side",
            LayoutPref::Stacked => "Stacked",
        }
    }
    pub fn value(self) -> &'static str {
        match self {
            LayoutPref::Side => "side",
            LayoutPref::Stacked => "stacked",
        }
    }
}

impl From<Layout> for LayoutPref {
    fn from(l: Layout) -> Self {
        match l {
            Layout::Side => LayoutPref::Side,
            Layout::Stacked => LayoutPref::Stacked,
        }
    }
}

// --- auto-update ---
/// `auto-update`: what the app does about new releases (see `crate::update`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutoUpdate {
    /// Download and verify in the background, then offer "Restart to update".
    #[default]
    Install,
    /// Only say a new release is available.
    Notify,
    /// Never check on its own (Check for Updates… still does).
    Off,
}

impl AutoUpdate {
    pub fn from_value(v: Option<&str>) -> Self {
        match v {
            Some("notify") => AutoUpdate::Notify,
            Some("off") => AutoUpdate::Off,
            _ => AutoUpdate::Install,
        }
    }
}

/// A font the user can pick. `family` is what GPUI resolves; `bundled` says
/// which embedded files (if any) must be registered first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FontChoice {
    pub label: &'static str,
    pub family: &'static str,
    pub bundled: Option<crate::fonts::Bundle>,
}

pub const WRITING_FONTS: &[FontChoice] = &[
    FontChoice {
        label: "Literata",
        family: "Literata",
        bundled: Some(crate::fonts::Bundle::Literata),
    },
    FontChoice {
        label: "Source Serif 4",
        family: "Source Serif 4",
        bundled: Some(crate::fonts::Bundle::SourceSerif),
    },
    FontChoice {
        label: "iA Writer Quattro",
        family: "iA Writer Quattro S",
        bundled: Some(crate::fonts::Bundle::Quattro),
    },
    FontChoice {
        label: "New York",
        family: "New York",
        bundled: None,
    },
    FontChoice {
        label: "Charter",
        family: "Charter",
        bundled: None,
    },
    FontChoice {
        label: "ET Book",
        family: "ETBembo",
        bundled: Some(crate::fonts::Bundle::EtBook),
    },
    FontChoice {
        label: "SF Pro",
        family: ".SystemUIFont",
        bundled: None,
    },
    FontChoice {
        label: "Inter",
        family: "Inter",
        bundled: Some(crate::fonts::Bundle::Inter),
    },
    FontChoice {
        label: "Menlo",
        family: "Menlo",
        bundled: None,
    },
];

pub const UI_FONTS: &[FontChoice] = &[
    FontChoice {
        label: "Inter",
        family: "Inter",
        bundled: Some(crate::fonts::Bundle::Inter),
    },
    FontChoice {
        label: "SF Pro",
        family: ".SystemUIFont",
        bundled: None,
    },
    FontChoice {
        label: "Literata",
        family: "Literata",
        bundled: Some(crate::fonts::Bundle::Literata),
    },
    FontChoice {
        label: "Source Serif 4",
        family: "Source Serif 4",
        bundled: Some(crate::fonts::Bundle::SourceSerif),
    },
    FontChoice {
        label: "iA Writer Quattro",
        family: "iA Writer Quattro S",
        bundled: Some(crate::fonts::Bundle::Quattro),
    },
    FontChoice {
        label: "New York",
        family: "New York",
        bundled: None,
    },
    FontChoice {
        label: "Charter",
        family: "Charter",
        bundled: None,
    },
    FontChoice {
        label: "ET Book",
        family: "ETBembo",
        bundled: Some(crate::fonts::Bundle::EtBook),
    },
    FontChoice {
        label: "Menlo",
        family: "Menlo",
        bundled: None,
    },
];

pub const DEFAULT_SIZE: f32 = 19.0;
pub const MIN_SIZE: f32 = 12.0;
pub const MAX_SIZE: f32 = 32.0;
pub const DEFAULT_HOTKEY: &str = "ctrl+alt+b";

#[derive(Debug, Clone, PartialEq)]
pub struct Prefs {
    /// A font label from `WRITING_FONTS` (or whatever the config says).
    pub writing_font: String,
    pub ui_font: String,
    pub font_size: f32,
    pub theme: ThemePref,
    pub layout: LayoutPref,
    pub hotkey: String,
    // --- scratch notes --- (read-only here: set in the config file)
    pub capture_default: CaptureDefault,
    pub new_note: NewNote,
    // --- buttons ---
    /// `show-buttons`: the title-bar toolbar and quick capture's button row.
    pub show_buttons: bool,
    // --- auto-update --- (read-only here: set in the config file)
    pub auto_update: AutoUpdate,
}

impl Default for Prefs {
    fn default() -> Self {
        Self::from_config(&Config::default())
    }
}

impl Prefs {
    pub fn from_config(c: &Config) -> Self {
        Self {
            writing_font: c.font_family_writing().to_string(),
            ui_font: c.font_family_ui().to_string(),
            font_size: clamp_size(c.font_size()),
            theme: c.theme().into(),
            layout: c.layout().into(),
            hotkey: c.capture_hotkey().to_string(),
            capture_default: c.capture_default(),
            new_note: c.new_note(),
            show_buttons: c.show_buttons(), // --- buttons ---
            auto_update: AutoUpdate::from_value(c.get("auto-update")), // --- auto-update ---
        }
    }

    /// The config keys to rewrite to get from `old` to `self`.
    pub fn changes_from(&self, old: &Prefs) -> Vec<(&'static str, Change)> {
        let mut out = Vec::new();
        if self.writing_font != old.writing_font {
            out.push((
                "font-family-writing",
                Change::Set(self.writing_font.clone()),
            ));
        }
        if self.ui_font != old.ui_font {
            out.push(("font-family-ui", Change::Set(self.ui_font.clone())));
        }
        if self.font_size != old.font_size {
            out.push((
                "font-size",
                Change::Set(blyg_core::config::parse::fmt_num(self.font_size)),
            ));
        }
        if self.theme != old.theme {
            out.push(("theme", Change::Set(self.theme.value().into())));
        }
        if self.layout != old.layout {
            out.push(("layout", Change::Set(self.layout.value().into())));
        }
        if self.hotkey != old.hotkey {
            out.push((
                "capture-hotkey",
                Change::Set(self.hotkey.trim().to_ascii_lowercase()),
            ));
        }
        // --- buttons ---
        if self.show_buttons != old.show_buttons {
            out.push(("show-buttons", Change::Set(self.show_buttons.to_string())));
        }
        out
    }

    pub fn writing(&self) -> FontChoice {
        find(WRITING_FONTS, &self.writing_font).unwrap_or(WRITING_FONTS[0])
    }

    pub fn ui(&self) -> FontChoice {
        find(UI_FONTS, &self.ui_font).unwrap_or(UI_FONTS[0])
    }

    pub fn bigger(&mut self) {
        self.font_size = clamp_size(self.font_size + 1.0);
    }
    pub fn smaller(&mut self) {
        self.font_size = clamp_size(self.font_size - 1.0);
    }
    pub fn reset_size(&mut self) {
        self.font_size = DEFAULT_SIZE;
    }

    /// List font size tracks the writing size the way the mock does (15 at 19).
    pub fn list_size(&self) -> f32 {
        (self.font_size * 15.0 / 19.0).round().clamp(11.0, 22.0)
    }
}

/// A font by label or family name, ignoring case.
pub fn find(list: &[FontChoice], name: &str) -> Option<FontChoice> {
    let n = name.trim();
    list.iter()
        .find(|f| f.label.eq_ignore_ascii_case(n) || f.family.eq_ignore_ascii_case(n))
        .copied()
}

fn clamp_size(s: f32) -> f32 {
    if s.is_finite() {
        s.clamp(MIN_SIZE, MAX_SIZE)
    } else {
        DEFAULT_SIZE
    }
}

/// Validate a hotkey string (`ctrl+alt+b`, `cmd+shift+space`, …).
pub fn parse_hotkey(s: &str) -> Option<global_hotkey::hotkey::HotKey> {
    s.trim().parse().ok()
}

/// Pretty form for display: `⌃⌥B`.
pub fn hotkey_glyphs(s: &str) -> String {
    let mut mods = String::new();
    let mut key = String::new();
    for part in s.split('+').map(str::trim) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods.push('⌃'),
            "alt" | "option" => mods.push('⌥'),
            "shift" => mods.push('⇧'),
            "cmd" | "command" | "super" | "meta" => mods.push('⌘'),
            "space" => key = "Space".into(),
            other => key = other.trim_start_matches("key").to_ascii_uppercase(),
        }
    }
    // Conventional macOS order: ⌃⌥⇧⌘
    let order = ['⌃', '⌥', '⇧', '⌘'];
    let mut sorted: String = order.iter().filter(|c| mods.contains(**c)).collect();
    sorted.push_str(&key);
    sorted
}

#[cfg(test)]
mod tests {
    use super::*;
    use blyg_core::ConfigStore;

    #[test]
    fn from_config_and_defaults() {
        let p = Prefs::default();
        assert_eq!(p.writing_font, "Literata");
        assert_eq!(p.ui_font, "Inter");
        assert_eq!(p.font_size, DEFAULT_SIZE);
        assert_eq!(p.theme, ThemePref::System);
        assert_eq!(p.layout, LayoutPref::Side);
        assert_eq!(p.hotkey, DEFAULT_HOTKEY);
        assert!(p.show_buttons, "buttons are on by default");
        let s = ConfigStore::in_memory(
            "font-size = 99\ntheme = dark\nlayout = stacked\nfont-family-ui = menlo\n",
        );
        let p = Prefs::from_config(s.config());
        assert_eq!(p.font_size, MAX_SIZE);
        assert_eq!(p.theme, ThemePref::Dark);
        assert_eq!(p.layout, LayoutPref::Stacked);
        assert_eq!(p.ui().family, "Menlo", "fonts match by label, any case");
    }

    #[test]
    fn only_changed_keys_are_written() {
        let mut s = ConfigStore::in_memory("# mine\ntheme = dark\n\n# fonts\nfont-size = 17\n");
        let old = Prefs::from_config(s.config());
        let mut new = old.clone();
        assert!(new.changes_from(&old).is_empty());
        new.bigger();
        new.layout = LayoutPref::Stacked;
        s.set(&new.changes_from(&old)).unwrap();
        assert_eq!(
            s.text().unwrap(),
            "# mine\ntheme = dark\n\n# fonts\nfont-size = 18\nlayout = stacked\n"
        );
        assert_eq!(Prefs::from_config(s.config()), new);
        // --- buttons ---
        let old = new.clone();
        new.show_buttons = false;
        s.set(&new.changes_from(&old)).unwrap();
        assert!(
            s.text()
                .unwrap()
                .ends_with("layout = stacked\nshow-buttons = false\n")
        );
        assert_eq!(Prefs::from_config(s.config()), new);
    }

    // --- auto-update ---
    #[test]
    fn auto_update_defaults_to_install() {
        assert_eq!(Prefs::default().auto_update, AutoUpdate::Install);
        for (text, want) in [
            ("auto-update = notify\n", AutoUpdate::Notify),
            ("auto-update = OFF\n", AutoUpdate::Off),
            ("auto-update = install\n", AutoUpdate::Install),
            ("auto-update = sometimes\n", AutoUpdate::Install),
        ] {
            let s = ConfigStore::in_memory(text);
            assert_eq!(Prefs::from_config(s.config()).auto_update, want, "{text}");
        }
    }

    #[test]
    fn font_lookup_falls_back() {
        let p = Prefs {
            writing_font: "Comic Sans".into(),
            ..Prefs::default()
        };
        assert_eq!(p.writing().label, "Literata");
        let p = Prefs {
            writing_font: "ET Book".into(),
            ..Prefs::default()
        };
        assert_eq!(p.writing().family, "ETBembo");
    }

    #[test]
    fn sizes_clamp() {
        let mut p = Prefs {
            font_size: MAX_SIZE,
            ..Prefs::default()
        };
        p.bigger();
        assert_eq!(p.font_size, MAX_SIZE);
        p.reset_size();
        assert_eq!(p.font_size, DEFAULT_SIZE);
        assert_eq!(p.list_size(), 15.0);
    }

    #[test]
    fn hotkeys() {
        assert!(parse_hotkey(DEFAULT_HOTKEY).is_some());
        assert!(parse_hotkey("ctrl+alt+B").is_some());
        assert!(parse_hotkey("cmd+shift+space").is_some());
        assert!(parse_hotkey("nonsense+").is_none());
        assert_eq!(hotkey_glyphs("ctrl+alt+b"), "⌃⌥B");
        assert_eq!(hotkey_glyphs("cmd+alt+ctrl+KeyK"), "⌃⌥⌘K");
    }
}
