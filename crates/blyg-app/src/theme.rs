//! Tufte palette, exactly the mock's `.win` / `.win.dark` CSS variables.

use gpui_kit::{Hsla, Rgba, WindowAppearance, rgb, rgba};

use crate::prefs::ThemePref;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub dark: bool,
    /// --wbg
    pub bg: Hsla,
    /// --wink
    pub ink: Hsla,
    /// --wmuted
    pub muted: Hsla,
    /// --wline
    pub line: Hsla,
    /// --wsel (selected list row)
    pub sel: Hsla,
    /// --wacc
    pub accent: Hsla,
    /// --wbar (title bar, status bar)
    pub bar: Hsla,
    /// Text selection in the editor: accent, translucent.
    pub text_selection: Hsla,
    pub warn: Hsla,
    pub over: Hsla,
    pub green: Hsla,
    pub amber: Hsla,
    pub grey: Hsla,
    pub ins_bg: Hsla,
    pub del_bg: Hsla,
    pub shadow: Hsla,
}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

fn ca(r: Rgba) -> Hsla {
    r.into()
}

impl Palette {
    pub fn light() -> Self {
        Self {
            dark: false,
            bg: c(0xfffff8),
            ink: c(0x111111),
            muted: c(0x8a867c),
            line: c(0xebe6d6),
            sel: c(0xf2ecd8),
            accent: c(0xa4271b),
            bar: c(0xf6f3e7),
            text_selection: ca(rgba(0xa4271b33)),
            warn: c(0xc98a00),
            over: c(0xd23c2a),
            green: c(0x3aa655),
            amber: c(0xe0a100),
            grey: c(0x999999),
            ins_bg: ca(rgba(0x3aa6552e)),
            del_bg: ca(rgba(0xd23c2a29)),
            shadow: ca(rgba(0x00000059)),
        }
    }

    pub fn dark() -> Self {
        Self {
            dark: true,
            bg: c(0x161513),
            ink: c(0xe4dfd3),
            muted: c(0x7f7a70),
            line: c(0x2a2824),
            sel: c(0x26231e),
            accent: c(0xe0775a),
            bar: c(0x1c1b18),
            text_selection: ca(rgba(0xe0775a40)),
            ..Self::light()
        }
    }

    pub fn resolve(pref: ThemePref, appearance: WindowAppearance) -> Self {
        let dark = match pref {
            ThemePref::Light => false,
            ThemePref::Dark => true,
            ThemePref::System => {
                matches!(
                    appearance,
                    WindowAppearance::Dark | WindowAppearance::VibrantDark
                )
            }
        };
        if dark { Self::dark() } else { Self::light() }
    }
}
