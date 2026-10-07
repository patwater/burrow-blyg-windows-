//! Themes in the UI. The files, the vocabulary and the resolution rules live
//! in `blyg_core::config::theme`; this module turns a resolved theme into a
//! [`Palette`] (every colour the views use) and a [`Theme`] (the palette plus
//! radii, fonts and the ornament slots `crate::ornament` draws).
//!
//! The registry is a GPUI global ([`Themes`]), loaded at launch from the
//! built-ins and `~/.config/blygger/themes/`, and reloaded with the config.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use blyg_core::config::theme::{
    Border, Family, Ornament, Registry, Resolved, Rgba as CoreRgba, Slot,
};
use gpui_kit::{App, Global, Hsla, Rgba, Styled, WindowAppearance, rgba};

use crate::prefs::{FontChoice, Prefs};

/// Every colour the views draw with. `Copy`, so a view takes `let p =
/// self.palette;` and closures capture it freely.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub dark: bool,
    pub bg: Hsla,
    pub ink: Hsla,
    pub muted: Hsla,
    pub line: Hsla,
    /// The selected list row.
    pub sel: Hsla,
    pub accent: Hsla,
    /// The title bar.
    pub bar: Hsla,
    /// Text selection in the editor.
    pub text_selection: Hsla,
    pub warn: Hsla,
    pub over: Hsla,
    pub green: Hsla,
    pub amber: Hsla,
    pub grey: Hsla,
    pub ins_bg: Hsla,
    pub del_bg: Hsla,
    pub shadow: Hsla,
    /// The editor's paper.
    pub editor: Hsla,
    /// Dividers between panes (see [`Rule`]).
    pub divider: Hsla,
    /// The posts list (sidebar) and its text.
    pub side: Hsla,
    pub side_ink: Hsla,
    pub side_muted: Hsla,
    pub side_accent: Hsla,
    pub side_line: Hsla,
    /// Text in the selected row.
    pub sel_ink: Hsla,
    pub sel_muted: Hsla,
    /// The window title and the title bar's lower edge.
    pub bar_ink: Hsla,
    pub bar_line: Hsla,
    pub status: Hsla,
    pub status_ink: Hsla,
    pub status_line: Hsla,
    pub quote_bg: Hsla,
    pub quote_rule: Hsla,
    pub toast: Hsla,
    pub toast_ink: Hsla,
    /// The reading list's "edited" badge and the rule beside edit notes.
    pub edited: Hsla,
    pub edited_bg: Hsla,
    pub edited_rule: Hsla,
    /// Notices in the reader (a withdrawn post).
    pub notice: Hsla,
    pub notice_bg: Hsla,
}

/// A core colour as GPUI's.
pub fn hsla(c: CoreRgba) -> Hsla {
    rgba(c.0).into()
}

/// `a` moved `t` of the way to `b` (in RGB).
pub fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (a.to_rgb(), b.to_rgb());
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
    .into()
}

/// WCAG contrast between two colours (alpha ignored).
pub fn contrast(a: Hsla, b: Hsla) -> f32 {
    let lum = |c: Hsla| {
        let r = c.to_rgb();
        let ch = |v: f32| {
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * ch(r.r) + 0.7152 * ch(r.g) + 0.0722 * ch(r.b)
    };
    let (x, y) = (lum(a), lum(b));
    let (hi, lo) = if x > y { (x, y) } else { (y, x) };
    (hi + 0.05) / (lo + 0.05)
}

impl Palette {
    pub fn from_resolved(t: &Resolved) -> Palette {
        let c = |n: &str| hsla(t.color(n));
        Palette {
            dark: t.is_dark(),
            bg: c("bg"),
            ink: c("ink"),
            muted: c("muted"),
            line: c("line"),
            sel: c("sel"),
            accent: c("accent"),
            bar: c("bar"),
            text_selection: c("text-selection"),
            warn: c("warn"),
            over: c("over"),
            green: c("green"),
            amber: c("amber"),
            grey: c("grey"),
            ins_bg: c("ins"),
            del_bg: c("del"),
            shadow: c("shadow"),
            editor: c("editor"),
            divider: c("divider"),
            side: c("side"),
            side_ink: c("side-ink"),
            side_muted: c("side-muted"),
            side_accent: c("side-accent"),
            side_line: c("side-line"),
            sel_ink: c("sel-ink"),
            sel_muted: c("sel-muted"),
            bar_ink: c("bar-ink"),
            bar_line: c("bar-line"),
            status: c("status"),
            status_ink: c("status-ink"),
            status_line: c("status-line"),
            quote_bg: c("quote-bg"),
            quote_rule: c("quote-rule"),
            toast: c("toast"),
            toast_ink: c("toast-ink"),
            edited: c("edited"),
            edited_bg: c("edited-bg"),
            edited_rule: c("edited-rule"),
            notice: c("notice"),
            notice_bg: c("notice-bg"),
        }
    }

    /// The palette for the sidebar: its ground, text and marks.
    pub fn on_side(&self) -> Palette {
        Palette {
            bg: self.side,
            ink: self.side_ink,
            muted: self.side_muted,
            accent: self.side_accent,
            line: self.side_line,
            ..*self
        }
    }

    /// The palette inside the selected row.
    pub fn on_sel(&self) -> Palette {
        let side = self.on_side();
        Palette {
            bg: self.sel,
            ink: self.sel_ink,
            muted: self.sel_muted,
            // The page's accent when it reads on the row, else the sidebar's.
            accent: if contrast(self.accent, self.sel) >= 3.0 {
                self.accent
            } else {
                side.accent
            },
            ..side
        }
    }

    /// The palette on a chrome surface (`surface` with `ink` on it): the
    /// same palette when body text already reads there, otherwise text,
    /// hover and selection derived from `ink` (a dark title bar).
    fn on_chrome(&self, surface: Hsla, ink: Hsla) -> Palette {
        if contrast(self.ink, surface) >= 4.5 {
            return Palette {
                muted: ink,
                ..*self
            };
        }
        Palette {
            bg: surface,
            ink,
            muted: ink.opacity(0.78),
            sel: ink.opacity(0.16),
            line: ink.opacity(0.3),
            accent: ink,
            ..*self
        }
    }

    /// The palette on the title bar (the toolbar and the view switcher).
    pub fn on_bar(&self) -> Palette {
        let mut p = self.on_chrome(self.bar, self.bar_ink);
        // The plain look keeps its muted title-bar labels, when they read
        // there; on a patterned or deeper bar they take the bar's own ink.
        if contrast(self.ink, self.bar) >= 4.5 {
            p.muted = if contrast(self.muted, self.bar) >= 4.5 {
                self.muted
            } else {
                self.bar_ink
            };
            // The active button's ground: `sel` when it shows on the bar,
            // else a faint wash of ink (Paper's and Ink's `sel` are nearly
            // their bar's colour).
            if contrast(self.sel, self.bar) < 1.15 {
                p.sel = mix(self.bar, self.ink, if self.dark { 0.14 } else { 0.09 });
            }
        }
        p
    }

    /// A tinted panel inside page content (cards, popovers, banners, the
    /// address field): the title bar's colour when page text reads on it,
    /// else a faint tint of the page, so a dark title bar never puts
    /// dark text on dark.
    pub fn panel(&self) -> Hsla {
        if contrast(self.ink, self.bar) >= 4.5 && contrast(self.muted, self.bar) >= 3.0 {
            self.bar
        } else {
            let toward = if self.dark {
                gpui_kit::white()
            } else {
                gpui_kit::black()
            };
            mix(self.bg, toward, 0.05)
        }
    }

    /// The palette on the status bar.
    pub fn on_status(&self) -> Palette {
        self.on_chrome(self.status, self.status_ink)
    }

    /// The selected-row surface for a list on the page (the Reader's post
    /// list, the stream, Subscriptions, versions, profiles): `sel` when page
    /// text reads on it and it stands apart from the page, else a tint of
    /// the page toward it (or toward the sidebar, when `sel` is the page's
    /// own colour). A theme's `sel` is chosen for its sidebar, so a dark
    /// lantern row (Konkan) would otherwise put dark text on dark.
    pub fn page_sel(&self) -> Hsla {
        let apart = contrast(self.sel, self.bg) >= 1.04;
        if apart && contrast(self.ink, self.sel) >= 4.5 && contrast(self.muted, self.sel) >= 3.0 {
            return self.sel;
        }
        let toward = if apart { self.sel } else { self.side };
        mix(self.bg, toward, if self.dark { 0.14 } else { 0.1 })
    }

    /// The palette for page content: `sel` is [`Palette::page_sel`], so
    /// hovered and selected rows keep page text readable.
    pub fn on_page(&self) -> Palette {
        Palette {
            sel: self.page_sel(),
            ..*self
        }
    }

    /// A small badge's text and ground (`new`, `edited`, `paused`): the
    /// accent on a wash of it, which reads on every theme (amber on a light
    /// row did not).
    pub fn badge(&self) -> (Hsla, Hsla) {
        let ground = mix(self.bg, self.accent, if self.dark { 0.18 } else { 0.12 });
        (self.accent, ground)
    }

    /// A recessed ground on the page (a segmented toggle's trough):
    /// [`Palette::page_sel`] when it stands apart from the page, else a
    /// faint wash of ink.
    pub fn well(&self) -> Hsla {
        let s = self.page_sel();
        if contrast(s, self.bg) >= 1.12 {
            s
        } else {
            mix(self.bg, self.ink, if self.dark { 0.1 } else { 0.07 })
        }
    }

    /// A quiet badge (`withdrawn`, `paused`): muted text on a wash of it.
    pub fn quiet_badge(&self) -> (Hsla, Hsla) {
        (self.muted, mix(self.bg, self.muted, 0.12))
    }

    /// The plain light theme (Paper).
    #[cfg(test)]
    pub fn light() -> Self {
        builtins().get("light").palette
    }

    /// The plain dark theme (Ink).
    #[cfg(test)]
    pub fn dark() -> Self {
        builtins().get("dark").palette
    }

    /// CSS custom properties for the reader's fallback stylesheet
    /// (`studio::style_cache::BUILTIN_CSS` and `studio::reader::reader_css`
    /// read these names): the page, its text, quote boxes, selection, and
    /// the diff and highlight colours, all from the palette.
    pub fn reader_css_vars(&self) -> String {
        let hex = |c: Hsla| {
            let r = c.to_rgb();
            let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            if r.a >= 0.999 {
                format!("#{:02x}{:02x}{:02x}", b(r.r), b(r.g), b(r.b))
            } else {
                format!(
                    "rgba({}, {}, {}, {:.3})",
                    b(r.r),
                    b(r.g),
                    b(r.b),
                    r.a.clamp(0.0, 1.0)
                )
            }
        };
        let paper = self.editor;
        format!(
            ":root {{ --paper: {}; --ink: {}; --muted: {}; --accent: {}; --rule: {}; \
             --wash: {}; --quote-bg: {}; --quote-rule: {}; --selection: {}; \
             --ins: {}; --del: {}; --over: {}; --mark: {}; \
             --alert: {}; --alert-wash: {}; color-scheme: {}; }}\n",
            hex(paper),
            hex(self.ink),
            hex(self.muted),
            hex(self.accent),
            hex(self.line),
            hex(mix(paper, self.ink, 0.05)),
            hex(self.quote_bg),
            hex(self.quote_rule),
            hex(self.text_selection),
            hex(self.ins_bg),
            hex(self.del_bg),
            hex(self.over),
            hex(self.accent.opacity(if self.dark { 0.3 } else { 0.18 })),
            hex(self.over),
            hex(self.del_bg),
            if self.dark { "dark" } else { "light" }
        )
    }
}

/// A resolved theme, ready to draw.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub id: String,
    pub name: String,
    pub family: Family,
    pub builtin: bool,
    pub palette: Palette,
    pub radius: f32,
    pub sheet_radius: f32,
    pub toast_radius: f32,
    pub chip_radius: f32,
    pub sheet_border: Border,
    pub chip_border: Border,
    pub toast_border: Border,
    /// Fonts the theme suggests (the user's `font-family-*` wins).
    pub font_writing: Option<FontChoice>,
    pub font_ui: Option<FontChoice>,
    pub font_chrome: Option<FontChoice>,
    resolved: Resolved,
}

impl Theme {
    pub fn from_resolved(r: Resolved) -> Theme {
        let find = |list: &[FontChoice], name: &Option<String>| {
            name.as_deref().and_then(|n| crate::prefs::find(list, n))
        };
        Theme {
            id: r.id.clone(),
            name: r.name.clone(),
            family: r.family,
            builtin: r.builtin,
            palette: Palette::from_resolved(&r),
            radius: r.radius,
            sheet_radius: r.sheet_radius,
            toast_radius: r.toast_radius,
            chip_radius: r.chip_radius,
            sheet_border: r.sheet_border,
            chip_border: r.chip_border,
            toast_border: r.toast_border,
            font_writing: find(crate::prefs::WRITING_FONTS, &r.font_writing),
            font_ui: find(crate::prefs::UI_FONTS, &r.font_ui),
            font_chrome: find(crate::prefs::UI_FONTS, &r.font_chrome),
            resolved: r,
        }
    }

    /// The ornament in `slot` (`none` when unset).
    pub fn slot(&self, slot: Slot) -> &Ornament {
        self.resolved.slot(slot)
    }

    pub fn has(&self, slot: Slot) -> bool {
        !self.slot(slot).is_none()
    }

    /// The font for the status bar, pills and hints: the theme's, unless the
    /// user chose an interface font; otherwise Inter.
    pub fn chrome_font(&self, prefs: &Prefs) -> &'static str {
        match self.font_chrome {
            Some(f) if !prefs.ui_font_set => f.family,
            _ => "Inter",
        }
    }

    /// A small corner (a button, a field, a banner) drawn at `v` points in
    /// the plain look, scaled by the theme's `radius` (8 in the plain look):
    /// square on a chart, soft on moss.
    pub fn corner(&self, v: f32) -> f32 {
        v * self.radius / 8.0
    }

    /// A few colours for a swatch preview: paper, sidebar, bar, accent, ink.
    pub fn swatch(&self) -> [Hsla; 5] {
        let p = &self.palette;
        [p.editor, p.side, p.bar, p.accent, p.ink]
    }
}

/// The loaded themes (a GPUI global): the registry plus every theme
/// resolved once.
pub struct Themes {
    pub registry: Registry,
    resolved: HashMap<String, Arc<Theme>>,
}

impl Global for Themes {}

impl Themes {
    pub fn new(registry: Registry) -> Themes {
        let resolved = registry
            .ids()
            .into_iter()
            .filter_map(|id| {
                let r = registry.resolve(&id)?;
                Some((id, Arc::new(Theme::from_resolved(r))))
            })
            .collect();
        Themes { registry, resolved }
    }

    /// Built-ins, plus the user's themes when `dir` is given.
    pub fn load(dir: Option<PathBuf>) -> Themes {
        Themes::new(match dir {
            Some(d) => Registry::load(&d),
            None => Registry::builtin(),
        })
    }

    /// A theme by id (never `system`); the plain light theme if unknown.
    pub fn get(&self, id: &str) -> Arc<Theme> {
        self.resolved
            .get(id)
            .or_else(|| self.resolved.get("light"))
            .cloned()
            .unwrap_or_else(|| {
                Arc::new(Theme::from_resolved(
                    Registry::builtin().pick("light", None, false),
                ))
            })
    }

    /// The theme `prefs` asks for under this appearance.
    pub fn pick(&self, prefs: &Prefs, appearance: WindowAppearance) -> Arc<Theme> {
        let dark = matches!(
            appearance,
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        );
        let id = self
            .registry
            .pick_id(&prefs.theme, prefs.theme_dark.as_deref(), dark);
        self.get(id)
    }

    /// Every theme, grouped for Settings: Plain, Woody, Oceanic (built-ins
    /// in their order, then the user's).
    pub fn grouped(&self) -> Vec<(Family, Vec<Arc<Theme>>)> {
        let ids = self.registry.ids();
        [Family::Plain, Family::Woody, Family::Oceanic]
            .into_iter()
            .map(|f| {
                let list = ids
                    .iter()
                    .map(|id| self.get(id))
                    .filter(|t| t.family == f)
                    .collect();
                (f, list)
            })
            .collect()
    }
}

/// The built-in themes alone (headless tests and windows opened before the
/// global exists).
pub fn builtins() -> &'static Themes {
    static B: OnceLock<Themes> = OnceLock::new();
    B.get_or_init(|| Themes::load(None))
}

/// Re-read the themes folder (with the config).
pub fn reload(cx: &mut App) {
    let dir = cx
        .try_global::<Themes>()
        .and_then(|t| t.registry.dir.clone());
    if dir.is_some() {
        cx.set_global(Themes::load(dir));
    }
}

/// The loaded themes (the built-ins when none were loaded).
pub fn themes(cx: &App) -> &Themes {
    cx.try_global::<Themes>().unwrap_or_else(|| builtins())
}

/// The theme for `prefs` in this window's appearance.
pub fn resolve(prefs: &Prefs, appearance: WindowAppearance, cx: &App) -> Arc<Theme> {
    themes(cx).pick(prefs, appearance)
}

/// The reader's CSS variables for the current theme (empty for the plain
/// built-ins, whose colours the fallback stylesheet already has).
static READER_VARS: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// The reader's text font: the writing font (the theme's, unless the
/// config sets one). `None`: the fallback stylesheet's own (Literata).
static READER_FONT: std::sync::Mutex<Option<FontChoice>> = std::sync::Mutex::new(None);

/// Remember the theme the reader's fallback stylesheet should take, and
/// the writing font (`Prefs::writing`, after `adopt_theme_fonts`) its text
/// is set in.
pub fn set_reader_theme(t: &Theme, writing: FontChoice) {
    let vars = if t.builtin && t.family == Family::Plain {
        String::new()
    } else {
        format!(
            "{}:root {{ --radius: {}px; }}\n{}",
            t.palette.reader_css_vars(),
            t.radius.min(12.),
            reader_quote_css(t)
        )
    };
    *READER_VARS.lock().unwrap_or_else(|e| e.into_inner()) = vars;
    *READER_FONT.lock().unwrap_or_else(|e| e.into_inner()) =
        (writing.family != "Literata").then_some(writing);
    // The same colours as `Palette::tip()` (theme_ext), which the toolbar
    // and composer tooltips use: toast ground, ink made to read on it.
    let (tip_bg, tip_ink) = t.palette.tip();
    *TIP.lock().unwrap_or_else(|e| e.into_inner()) = Some(TipStyle {
        bg: tip_bg,
        ink: tip_ink,
        font: t.font_chrome.map(|f| f.family).unwrap_or("Inter"),
        radius: t.toast_radius.min(6.),
    });
}

/// How tooltips look: the theme's toast colours, its chrome font and a
/// corner no rounder than its toasts'.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TipStyle {
    pub bg: Hsla,
    pub ink: Hsla,
    pub font: &'static str,
    pub radius: f32,
}

static TIP: std::sync::Mutex<Option<TipStyle>> = std::sync::Mutex::new(None);

/// The tooltip style [`set_reader_theme`] chose (the plain look's dark
/// pill before any theme is applied).
pub fn tip_style() -> TipStyle {
    (*TIP.lock().unwrap_or_else(|e| e.into_inner())).unwrap_or(TipStyle {
        bg: gpui_kit::black().opacity(0.85),
        ink: gpui_kit::white(),
        font: "Inter",
        radius: 5.,
    })
}

/// The reader's quote boxes in the shape of the theme's `quote.frame`, as
/// the stream draws them natively (a chart's double rule, a sashiko stitch,
/// a buoy's top edge…). Empty for the plain left rule.
pub fn reader_quote_css(t: &Theme) -> String {
    use blyg_core::config::theme::Kind;
    let o = t.slot(Slot::QuoteFrame);
    let rule = o.color(0).map(hsla).unwrap_or(t.palette.quote_rule);
    let r = rule.to_rgb();
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let c = format!("#{:02x}{:02x}{:02x}", b(r.r), b(r.g), b(r.b));
    let rad = t.radius.min(12.);
    let body = match o.kind {
        Kind::Arch => format!("border: 1px solid {c}; border-radius: 14px 14px 6px 6px;"),
        Kind::Cushion => format!("border: 2px solid {c}; border-radius: 22px 12px 18px 10px;"),
        Kind::Outline | Kind::BoxCorners => {
            format!("border: 1px solid {c}; border-radius: {rad}px;")
        }
        Kind::InsetBar => format!("border: 0; border-left: 3px solid {c}; border-radius: 0;"),
        Kind::Buoy => format!(
            "border: 0; border-top: 2px solid {c}; border-radius: 1px; \
             box-shadow: 0 0 0 1px var(--rule), 2px 2px 0 1px var(--rule);"
        ),
        Kind::DoubleRule => format!("border: 3px double {c}; border-radius: 0;"),
        Kind::Sashiko => format!(
            "border: 1px dashed {c}; border-radius: 3px; \
             box-shadow: 0 0 0 3px var(--quote-bg);"
        ),
        _ => return String::new(),
    };
    format!("blockquote {{ {body} }}\n")
}

/// The font [`set_reader_theme`] chose (`None`: Literata).
pub fn reader_font() -> Option<FontChoice> {
    *READER_FONT.lock().unwrap_or_else(|e| e.into_inner())
}

/// The CSS [`set_reader_theme`] chose.
pub fn reader_vars() -> String {
    READER_VARS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// A divider: one hairline in the theme's divider colour. Every pane edge
/// goes through these, so a theme recolours them all in one place.
pub trait Rule: Styled + Sized {
    fn rule_b(self, p: &Palette) -> Self {
        self.border_b_1().border_color(p.divider)
    }
    fn rule_t(self, p: &Palette) -> Self {
        self.border_t_1().border_color(p.divider)
    }
    fn rule_l(self, p: &Palette) -> Self {
        self.border_l_1().border_color(p.divider)
    }
    fn rule_r(self, p: &Palette) -> Self {
        self.border_r_1().border_color(p.divider)
    }
}

impl<T: Styled> Rule for T {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paper_and_ink_keep_the_original_palettes() {
        let l = Palette::light();
        assert!(!l.dark);
        assert_eq!(l.bg, hsla(CoreRgba(0xfffff8ff)));
        assert_eq!(l.accent, hsla(CoreRgba(0xa4271bff)));
        assert_eq!(l.side, l.bg, "the plain list sits on the page");
        assert_eq!(l.editor, l.bg);
        let d = Palette::dark();
        assert!(d.dark);
        assert_eq!(d.bg, hsla(CoreRgba(0x161513ff)));
        assert_eq!(d.warn, l.warn);
    }

    #[test]
    fn every_builtin_resolves_with_known_fonts() {
        let t = builtins();
        for id in t.registry.ids() {
            let th = t.get(&id);
            assert_eq!(th.id, id);
            let r = t.registry.resolve(&id).unwrap();
            for (name, got) in [
                (&r.font_writing, th.font_writing),
                (&r.font_ui, th.font_ui),
                (&r.font_chrome, th.font_chrome),
            ] {
                assert_eq!(name.is_some(), got.is_some(), "{id}: font {name:?} unknown");
            }
        }
        assert_eq!(t.get("portolan").font_writing.unwrap().family, "ETBembo");
        assert_eq!(t.get("fortress").font_ui.unwrap().family, "Menlo");
        let groups = t.grouped();
        let names = |f: usize| groups[f].1.iter().map(|t| t.id.clone()).collect::<Vec<_>>();
        assert_eq!(names(0), ["light", "dark"]);
        assert_eq!(names(1), ["cutaway", "kumiko", "shola", "fortress"]);
        assert_eq!(names(2), ["portolan", "aizome", "saltspace", "konkan"]);
    }

    #[test]
    fn pick_uses_theme_dark_in_dark_mode() {
        let t = builtins();
        let mut p = Prefs {
            theme: "cutaway".into(),
            theme_dark: Some("fortress".into()),
            ..Prefs::default()
        };
        assert_eq!(t.pick(&p, WindowAppearance::Light).id, "cutaway");
        assert_eq!(t.pick(&p, WindowAppearance::Dark).id, "fortress");
        p.theme = "system".into();
        p.theme_dark = None;
        assert_eq!(t.pick(&p, WindowAppearance::VibrantDark).id, "dark");
        assert_eq!(t.pick(&p, WindowAppearance::VibrantLight).id, "light");
    }

    /// Every surface that paints its own ground must carry text that reads
    /// on it: the sidebars (Posts and the Reader's subscriptions), the
    /// selected row, the title bar's labels, the status bar, and the tinted
    /// panels inside pages (a dark title bar once gave dark-on-dark here).
    #[test]
    fn text_reads_on_every_surface_of_every_builtin() {
        let t = builtins();
        let mut bad = Vec::new();
        for id in t.registry.ids() {
            let p = t.get(&id).palette;
            let mut need = |what: &str, fg: Hsla, bg: Hsla, min: f32| {
                let c = contrast(fg, bg);
                if c < min {
                    bad.push(format!("{id}: {what} {c:.2} < {min}"));
                }
            };
            need("page ink", p.ink, p.bg, 4.5);
            let side = p.on_side();
            need("sidebar ink", side.ink, side.bg, 4.5);
            need("sidebar muted", side.muted, side.bg, 3.0);
            let sel = p.on_sel();
            need("selected-row ink", sel.ink, sel.bg, 4.5);
            let bar = p.on_bar();
            need("title-bar ink", bar.ink, p.bar, 4.5);
            need("title-bar labels", bar.muted, p.bar, 3.0);
            let st = p.on_status();
            need("status-bar ink", st.ink, p.status, 4.5);
            need("panel ink", p.ink, p.panel(), 4.5);
            need("panel muted", p.muted, p.panel(), 3.0);
            // --- the reading side ---
            // Lists on the page (the Reader's posts, the stream,
            // Subscriptions, versions, profiles): the selected ground.
            let ps = p.page_sel();
            need("page-selected ink", p.ink, ps, 4.5);
            need("page-selected muted", p.muted, ps, 3.0);
            need("page-selected accent", p.accent, ps, 3.0);
            // Stands apart from the page (1.0 would be invisible).
            need("page-selected ground vs page", ps, p.bg, 1.03);
            need("toggle trough muted", p.muted, p.well(), 3.0);
            need("links on the page", p.accent, p.bg, 3.0);
            let (fg, bg) = p.badge();
            need("new badge", fg, bg, 3.0);
            let (fg, bg) = p.quiet_badge();
            need("quiet badge", fg, bg, 3.0);
            need("edited badge", p.edited, over(p.edited_bg, p.bg), 3.0);
            need("notice banner", p.notice, over(p.notice_bg, p.bg), 3.0);
            need("pin label on the page", p.notice, p.bg, 3.0);
            // Stream quote boxes: the quoted text and the footer.
            need("quote ink", p.ink, p.quote_bg, 4.5);
            need("quote footer", p.muted, p.quote_bg, 3.0);
            // Toasts and tooltips; the title bar's active button; a theme
            // swatch's edge on this theme's sheets.
            need("toast / tooltip ink", p.toast_ink, p.toast, 4.5);
            need("title-bar active ground", bar.sel, p.bar, 1.1);
            need("swatch edge", crate::ornament::edge_on(p.bg), p.bg, 1.5);
            // The reader's WebView paper.
            need("reader ink", p.ink, p.editor, 4.5);
            need("reader muted", p.muted, p.editor, 3.0);
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    /// A translucent `c` laid over an opaque `ground`.
    fn over(c: Hsla, ground: Hsla) -> Hsla {
        mix(ground, c.opacity(1.0), c.a)
    }

    #[test]
    fn reader_css_takes_the_palette() {
        let css = builtins().get("aizome").palette.reader_css_vars();
        assert!(css.contains("--paper: #f6f3ea"), "{css}");
        assert!(css.contains("--ink: #1c2340"), "{css}");
        assert!(css.contains("color-scheme: light"));
        // Quote boxes and diff marks from the palette, not generic.
        assert!(css.contains("--quote-bg: #eef0f6"), "{css}");
        assert!(css.contains("--quote-rule: #1f2c5c"), "{css}");
        assert!(css.contains("--selection: rgba("), "{css}");
        assert!(css.contains("--ins: rgba("), "{css}");
        // The reader's quote boxes take the theme's `quote.frame` shape.
        let t = builtins();
        assert!(reader_quote_css(&t.get("portolan")).contains("3px double #cdb887"));
        assert!(reader_quote_css(&t.get("aizome")).contains("1px dashed #1f2c5c"));
        assert!(reader_quote_css(&t.get("light")).is_empty());
    }
}
