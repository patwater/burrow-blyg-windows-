//! --- themes --- Chrome surfaces: the colours and shapes that sheets,
//! popovers, menus, fields, chips, key caps, tooltips and scrims take from
//! a theme. `theme.rs` owns the palette itself; these helpers derive what
//! the chrome needs from it, so that every theme (a dark selected row, a
//! laterite status bar, a pale accent) still gives text that reads:
//! at least 4.5:1 for text and 3:1 for secondary marks.

use gpui_kit::base::input::InputEditorStyle;
use gpui_kit::{BoxShadow, Hsla, Styled, black, point, px, white};

use crate::theme::{Palette, Theme, contrast, mix};

/// Body text.
pub const TEXT: f32 = 4.5;
/// Secondary labels, icons and component edges that carry meaning.
pub const MARK: f32 = 3.0;

/// `fg` if it reads on `bg` at `min`; otherwise `fg` moved toward black or
/// white (whichever is further from `bg`) just far enough.
pub fn readable(fg: Hsla, bg: Hsla, min: f32) -> Hsla {
    if contrast(fg, bg) >= min {
        return fg;
    }
    let target = if contrast(white(), bg) >= contrast(black(), bg) {
        white()
    } else {
        black()
    };
    for i in 1..=40 {
        let c = mix(fg, target, i as f32 / 40.);
        if contrast(c, bg) >= min {
            return c;
        }
    }
    target
}

/// `bg` nudged toward `ink` until the two grounds differ by `min` (a hover
/// or a chosen row that must show against the surface around it).
fn tint(bg: Hsla, ink: Hsla, min: f32) -> Hsla {
    for i in 1..=40 {
        let c = mix(bg, ink, i as f32 / 100.);
        if contrast(c, bg) >= min {
            return c;
        }
    }
    mix(bg, ink, 0.4)
}

impl Palette {
    /// The accent as text on the page (links, a chosen chip).
    pub fn accent_text(&self) -> Hsla {
        readable(self.accent, self.bg, TEXT)
    }

    /// Error text on the page.
    pub fn over_text(&self) -> Hsla {
        readable(self.over, self.bg, TEXT)
    }

    /// Warning text on the page (`warn` is a pale amber in light themes).
    pub fn warn_text(&self) -> Hsla {
        readable(self.warn, self.bg, TEXT)
    }

    /// Success text on the page.
    pub fn green_text(&self) -> Hsla {
        readable(self.green, self.bg, TEXT)
    }

    /// `fg` as text on the status bar's ground.
    pub fn on_status_text(&self, fg: Hsla) -> Hsla {
        readable(fg, self.status, TEXT)
    }

    /// The chosen row in a menu, popup or palette on the page. The theme's
    /// `sel` when page text reads on it; else a tint of the page (Konkan's
    /// selected row is a dark lantern, meant for the sidebar's light text).
    pub fn pick(&self) -> Hsla {
        // Measured against the raised ground too, so it shows in a popover.
        let raised = self.raised();
        if contrast(self.ink, self.sel) >= TEXT
            && contrast(self.sel, self.bg) >= 1.08
            && contrast(self.sel, raised) >= 1.08
        {
            self.sel
        } else {
            tint(raised, self.ink, 1.12)
        }
    }

    /// A hover ground on the page or a popover: visible, fainter than
    /// [`Palette::pick`].
    pub fn hover(&self) -> Hsla {
        tint(self.raised(), self.ink, 1.07)
    }

    /// The edge of a field, chip, popover or sheet on the page: a hairline
    /// that still shows on a dark ground.
    pub fn edge(&self) -> Hsla {
        readable(self.line, self.bg, 1.5)
    }

    /// The ground of a floating menu or popup: the page, lifted a little on
    /// a dark theme so it separates from the page under it.
    pub fn raised(&self) -> Hsla {
        if self.dark {
            mix(self.bg, white(), 0.05)
        } else {
            self.bg
        }
    }

    /// A drop shadow that shows on dark grounds too.
    pub fn drop(&self) -> Hsla {
        if self.dark {
            black().opacity(0.6)
        } else {
            self.shadow
        }
    }

    /// The veil behind a modal card: always a darkening (a light veil
    /// over a dark theme washes it out).
    pub fn scrim(&self) -> Hsla {
        black().opacity(if self.dark { 0.45 } else { 0.14 })
    }

    /// The text caret: the accent when it shows on the page, else ink.
    pub fn caret(&self) -> Hsla {
        if contrast(self.accent, self.bg) >= MARK {
            self.accent
        } else {
            self.ink
        }
    }

    /// Placeholder text in a field (a secondary label, 3:1 or better).
    pub fn placeholder(&self) -> Hsla {
        readable(self.muted, self.bg, MARK)
    }

    /// How a single-line field on the page draws (transparent: the field
    /// box behind it gives the ground).
    pub fn field(&self) -> InputEditorStyle {
        InputEditorStyle {
            foreground: self.ink,
            muted_foreground: self.placeholder(),
            background: gpui_kit::transparent_black(),
            border: self.edge(),
            selection: self.text_selection,
            caret: self.caret(),
            ..Default::default()
        }
    }

    /// A tooltip's ground and text: the theme's toast colours.
    pub fn tip(&self) -> (Hsla, Hsla) {
        (self.toast, readable(self.toast_ink, self.toast, TEXT))
    }
}

/// A sheet's box: the page's ground and ink, the theme's `sheet.border`
/// and `sheet.radius` (square top: it hangs from the title bar), and a
/// shadow that shows on dark themes.
pub fn sheet<E: Styled>(e: E, t: &Theme) -> E {
    let p = &t.palette;
    // The edge and the drop shadow go together: a double rule is drawn as
    // rings in the same shadow list, so a later `.shadow()` would erase it.
    crate::ornament::border_with_shadow(
        e.bg(p.bg).text_color(p.ink),
        t.sheet_border,
        p.edge(),
        p.bg,
        Some(BoxShadow {
            color: p.drop(),
            offset: point(px(0.), px(18.)),
            blur_radius: px(40.),
            spread_radius: px(-12.),
            inset: false,
        }),
    )
    .border_t_0()
    .rounded_b(px(t.sheet_radius))
}

/// A floating card (onboarding, the tour): like a sheet, rounded all round.
pub fn card<E: Styled>(e: E, t: &Theme) -> E {
    let p = &t.palette;
    crate::ornament::border_with_shadow(
        e.bg(p.bg).text_color(p.ink),
        t.sheet_border,
        p.edge(),
        p.bg,
        Some(BoxShadow {
            color: p.drop(),
            offset: point(px(0.), px(16.)),
            blur_radius: px(44.),
            spread_radius: px(-10.),
            inset: false,
        }),
    )
    .rounded(px(t.sheet_radius))
}

/// A menu or popup over the page: raised ground, a visible edge, the
/// theme's radius and a shadow that falls below.
pub fn popover<E: Styled>(e: E, t: &Theme) -> E {
    let p = &t.palette;
    e.bg(p.raised())
        .text_color(p.ink)
        .border_1()
        .border_color(p.edge())
        .rounded(px(t.radius.min(10.)))
        .shadow(vec![BoxShadow {
            color: p.drop().opacity(if p.dark { 0.7 } else { 0.45 }),
            offset: point(px(0.), px(8.)),
            blur_radius: px(18.),
            spread_radius: px(-6.),
            inset: false,
        }])
}

/// A chip (a choice, a small button): `chip.radius`, `chip.border`, and
/// the accent (as readable text) when it's the chosen one.
pub fn chip<E: Styled>(e: E, t: &Theme, on: bool) -> E {
    let p = &t.palette;
    let edge = if on { p.accent } else { p.edge() };
    let e = crate::ornament::border(e, t.chip_border, edge).rounded(px(t.chip_radius));
    if on { e.text_color(p.accent_text()) } else { e }
}

/// A key cap ("⏎", "esc").
pub fn kbd<E: Styled>(e: E, t: &Theme) -> E {
    let p = &t.palette;
    e.rounded(px(t.chip_radius.min(5.)))
        .border_1()
        .border_b_2()
        .border_color(p.edge())
        .text_color(p.ink)
}

/// The box around a text field: the theme's radius and a visible edge.
pub fn field_box<E: Styled>(e: E, t: &Theme) -> E {
    let p = &t.palette;
    e.rounded(px(t.radius.min(8.)))
        .border_1()
        .border_color(p.edge())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::builtins;

    /// Every chrome pair these helpers hand out reads in every built-in:
    /// text 4.5:1, secondary marks 3:1, and hover / chosen grounds that
    /// show against the page.
    #[test]
    fn chrome_pairs_read_in_every_builtin() {
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
            need("accent text", p.accent_text(), p.bg, TEXT);
            need("error text", p.over_text(), p.bg, TEXT);
            need("warning text", p.warn_text(), p.bg, TEXT);
            need("success text", p.green_text(), p.bg, TEXT);
            need("muted on page", p.muted, p.bg, MARK);
            need("placeholder", p.placeholder(), p.bg, MARK);
            need("caret", p.caret(), p.bg, MARK);
            need("ink on chosen row", p.ink, p.pick(), TEXT);
            need("muted on chosen row", p.muted, p.pick(), MARK);
            need("ink on hover", p.ink, p.hover(), TEXT);
            need("chosen row shows", p.pick(), p.bg, 1.08);
            need("chosen row shows in a popover", p.pick(), p.raised(), 1.08);
            need("hover shows", p.hover(), p.bg, 1.05);
            need("hover shows in a popover", p.hover(), p.raised(), 1.05);
            need("edge shows", p.edge(), p.bg, 1.5);
            need("ink on popover", p.ink, p.raised(), TEXT);
            need("muted on popover", p.muted, p.raised(), MARK);
            need("ink on panel", p.ink, p.panel(), TEXT);
            let (tip, tip_ink) = p.tip();
            need("tooltip", tip_ink, tip, TEXT);
            need("toast", p.toast_ink, p.toast, TEXT);
            let st = p.on_status();
            need("status text", st.muted, p.status, MARK);
            need("status error", p.on_status_text(p.over), p.status, TEXT);
            need("status warning", p.on_status_text(p.warn), p.status, TEXT);
            need("status link", p.on_status_text(p.accent), p.status, TEXT);
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    #[test]
    fn readable_keeps_colours_that_already_read() {
        let p = builtins().get("light").palette;
        assert_eq!(readable(p.ink, p.bg, TEXT), p.ink);
        assert_eq!(p.accent_text(), p.accent);
        // Konkan's lantern row is dark: menus on the page don't use it.
        let k = builtins().get("konkan").palette;
        assert_ne!(k.pick(), k.sel);
    }
}
