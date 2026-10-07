//! Themes: plain-text `key = value` files in the config syntax (see
//! [`super::parse`]), built in or in `~/.config/blygger/themes/<name>`.
//!
//! ```text
//! # plain, woody or oceanic; light or dark; inherit is optional
//! name        = My burrow
//! family      = woody
//! base        = light
//! inherit     = cutaway
//! color-bg    = #fbf3e2
//! font-writing = Literata
//! radius      = 8
//! sidebar.ground = strata(#7a8f45 #5b3d25)
//! sidebar.top    = roots(#d9b98a, opacity=0.55)
//! empty.art      = svg(~/.config/blygger/themes/art/burrow.svg)
//! ```
//!
//! A theme is resolved in layers: the base's plain theme (`light` or
//! `dark`), then the built-in named by `inherit`, then the file itself.
//! Colours a theme leaves unset are derived from others (the sidebar from
//! the background, and so on; see [`COLORS`]). Ornament slots take a word
//! from a fixed vocabulary ([`Ornament`]), with optional colours and
//! `key=value` arguments. Problems are [`Diagnostic`]s with line numbers,
//! like the config file's; a bad line never stops the rest from loading.
//!
//! This module knows nothing about drawing: colours are `0xRRGGBBAA`, fonts
//! are names (the app checks them), and ornaments are data.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::parse::{Diagnostic, Severity, parse_text, resolve_path};

// ------------------------------------------------------------------ built-ins

/// The built-in themes, in the order Settings lists them, as theme files.
pub const BUILTIN: &[(&str, &str)] = &[
    ("light", include_str!("../../themes/light")),
    ("dark", include_str!("../../themes/dark")),
    ("cutaway", include_str!("../../themes/cutaway")),
    ("kumiko", include_str!("../../themes/kumiko")),
    ("shola", include_str!("../../themes/shola")),
    ("fortress", include_str!("../../themes/fortress")),
    ("portolan", include_str!("../../themes/portolan")),
    ("aizome", include_str!("../../themes/aizome")),
    ("saltspace", include_str!("../../themes/saltspace")),
    ("konkan", include_str!("../../themes/konkan")),
];

/// The text of a built-in theme file.
pub fn builtin_text(id: &str) -> Option<&'static str> {
    BUILTIN.iter().find(|(n, _)| *n == id).map(|(_, t)| *t)
}

/// `theme = system` follows macOS between these two.
pub const SYSTEM: &str = "system";

/// A theme name: lowercase kebab-case (it's also a file name).
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
}

// ------------------------------------------------------------------ values

/// `0xRRGGBBAA`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgba(pub u32);

impl Rgba {
    pub fn rgb(self) -> u32 {
        self.0 >> 8
    }
    pub fn alpha(self) -> u8 {
        (self.0 & 0xff) as u8
    }
    pub fn with_alpha(self, a: u8) -> Rgba {
        Rgba((self.0 & !0xff) | a as u32)
    }
    /// `#rrggbb`, or `#rrggbbaa` when not opaque.
    pub fn hex(self) -> String {
        if self.alpha() == 0xff {
            format!("#{:06x}", self.rgb())
        } else {
            format!("#{:08x}", self.0)
        }
    }
    /// WCAG relative luminance (alpha ignored).
    pub fn luminance(self) -> f64 {
        let ch = |shift: u32| {
            let c = ((self.0 >> shift) & 0xff) as f64 / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * ch(24) + 0.7152 * ch(16) + 0.0722 * ch(8)
    }
    /// WCAG contrast ratio between two opaque colours (1 to 21).
    pub fn contrast(self, other: Rgba) -> f64 {
        let (a, b) = (self.luminance(), other.luminance());
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }
}

/// `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`.
pub fn parse_color(s: &str) -> Result<Rgba, String> {
    let t = s.trim();
    let hex = t
        .strip_prefix('#')
        .ok_or_else(|| format!("`{t}` isn't a colour (write #rrggbb)"))?;
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("`{t}` isn't a colour (write #rrggbb)"));
    }
    let expand = |h: &str| h.chars().flat_map(|c| [c, c]).collect::<String>();
    let full = match hex.len() {
        3 => expand(hex) + "ff",
        4 => expand(hex),
        6 => format!("{hex}ff"),
        8 => hex.to_string(),
        _ => return Err(format!("`{t}` isn't a colour (write #rrggbb)")),
    };
    u32::from_str_radix(&full, 16)
        .map(Rgba)
        .map_err(|_| format!("`{t}` isn't a colour"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Family {
    #[default]
    Plain,
    Woody,
    Oceanic,
}

impl Family {
    pub fn label(self) -> &'static str {
        match self {
            Family::Plain => "Plain",
            Family::Woody => "Woody",
            Family::Oceanic => "Oceanic",
        }
    }
    fn parse(s: &str) -> Option<Family> {
        match s {
            "plain" => Some(Family::Plain),
            "woody" => Some(Family::Woody),
            "oceanic" => Some(Family::Oceanic),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Base {
    #[default]
    Light,
    Dark,
}

impl Base {
    pub fn id(self) -> &'static str {
        match self {
            Base::Light => "light",
            Base::Dark => "dark",
        }
    }
}

/// How a sheet, a chip or a toast draws its edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Border {
    Solid,
    Dashed,
    /// Two hairlines with a gap (a chart's double rule).
    Double,
    None,
}

impl Border {
    fn parse(s: &str) -> Option<Border> {
        match s {
            "solid" => Some(Border::Solid),
            "dashed" => Some(Border::Dashed),
            "double" => Some(Border::Double),
            "none" => Some(Border::None),
            _ => None,
        }
    }
}

// ------------------------------------------------------------------ colours

/// What an unset colour falls back to.
#[derive(Debug, Clone, Copy)]
pub enum Fallback {
    /// Every base theme sets it.
    Required,
    /// The same as another colour.
    Same(&'static str),
    /// Another colour at this alpha.
    Alpha(&'static str, u8),
}

/// One themeable colour: `color-<name>` in a theme file.
#[derive(Debug, Clone, Copy)]
pub struct ColorKey {
    pub name: &'static str,
    pub fallback: Fallback,
    pub docs: &'static str,
}

const fn ck(name: &'static str, fallback: Fallback, docs: &'static str) -> ColorKey {
    ColorKey {
        name,
        fallback,
        docs,
    }
}

use Fallback::{Alpha, Required, Same};

/// Every colour, in the order theme files and docs list them. A fallback
/// always names a colour earlier in the list.
pub const COLORS: &[ColorKey] = &[
    ck(
        "bg",
        Required,
        "The window background, sheets and the reader.",
    ),
    ck("ink", Required, "Body text."),
    ck("muted", Required, "Secondary text: dates, hints, labels."),
    ck("line", Required, "Hairlines and borders."),
    ck(
        "accent",
        Required,
        "Links, the caret, highlights, the primary button.",
    ),
    ck("sel", Required, "The selected row in a list."),
    ck("bar", Required, "The title bar."),
    ck("warn", Required, "Near the length limit."),
    ck("over", Required, "Over the limit, errors, deletions."),
    ck("green", Required, "Synced."),
    ck("amber", Required, "Waiting to sync, warnings."),
    ck("grey", Required, "Not connected."),
    ck("shadow", Required, "Sheet and toast shadows."),
    ck(
        "edited",
        Required,
        "The \"edited\" badge on a reading-list post.",
    ),
    ck("edited-bg", Required, "The \"edited\" badge's background."),
    ck(
        "notice",
        Required,
        "Notices in the reader (a withdrawn post).",
    ),
    ck("notice-bg", Required, "Those notices' background."),
    ck(
        "edited-rule",
        Same("edited"),
        "The rule beside an author's edit notes.",
    ),
    ck("editor", Same("bg"), "The editor's paper."),
    ck("text-selection", Alpha("accent", 0x33), "Selected text."),
    ck("ins", Alpha("green", 0x2e), "Inserted text in a diff."),
    ck("del", Alpha("over", 0x29), "Deleted text in a diff."),
    ck("divider", Same("line"), "Dividers between panes."),
    ck("side", Same("bg"), "The posts list (the sidebar)."),
    ck("side-ink", Same("ink"), "Text in the sidebar."),
    ck(
        "side-muted",
        Same("muted"),
        "Secondary text in the sidebar.",
    ),
    ck(
        "side-accent",
        Same("accent"),
        "Search matches and marks in the sidebar.",
    ),
    ck("side-line", Same("divider"), "The sidebar's edge."),
    ck("sel-ink", Same("side-ink"), "Text in the selected row."),
    ck(
        "sel-muted",
        Same("side-muted"),
        "Secondary text in the selected row.",
    ),
    ck("bar-ink", Same("muted"), "The window title."),
    ck("bar-line", Same("divider"), "The title bar's lower edge."),
    ck("status", Same("bar"), "The status bar."),
    ck("status-ink", Same("muted"), "Text in the status bar."),
    ck(
        "status-line",
        Same("divider"),
        "The status bar's upper edge.",
    ),
    ck("quote-bg", Same("sel"), "A quote box."),
    ck("quote-rule", Same("accent"), "A quote box's rule."),
    ck("toast", Same("ink"), "A toast."),
    ck("toast-ink", Same("bg"), "Text in a toast."),
];

pub fn color_key(name: &str) -> Option<&'static ColorKey> {
    COLORS.iter().find(|c| c.name == name)
}

// ------------------------------------------------------------------ slots

/// Where an ornament can go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Slot {
    TitlebarBand,
    SidebarGround,
    SidebarTop,
    RowSelected,
    EditorFrame,
    QuoteFrame,
    Divider,
    StatusOrnament,
    MarkerPinned,
    MarkerNew,
    Texture,
    ScrollEdge,
    EmptyArt,
}

impl Slot {
    pub const ALL: [Slot; 13] = [
        Slot::TitlebarBand,
        Slot::SidebarGround,
        Slot::SidebarTop,
        Slot::RowSelected,
        Slot::EditorFrame,
        Slot::QuoteFrame,
        Slot::Divider,
        Slot::StatusOrnament,
        Slot::MarkerPinned,
        Slot::MarkerNew,
        Slot::Texture,
        Slot::ScrollEdge,
        Slot::EmptyArt,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Slot::TitlebarBand => "titlebar.band",
            Slot::SidebarGround => "sidebar.ground",
            Slot::SidebarTop => "sidebar.top",
            Slot::RowSelected => "row.selected",
            Slot::EditorFrame => "editor.frame",
            Slot::QuoteFrame => "quote.frame",
            Slot::Divider => "divider",
            Slot::StatusOrnament => "status.ornament",
            Slot::MarkerPinned => "marker.pinned",
            Slot::MarkerNew => "marker.new",
            Slot::Texture => "texture",
            Slot::ScrollEdge => "scroll.edge",
            Slot::EmptyArt => "empty.art",
        }
    }

    pub fn docs(self) -> &'static str {
        match self {
            Slot::TitlebarBand => "Behind the title bar.",
            Slot::SidebarGround => "Behind the posts list.",
            Slot::SidebarTop => "Along the top of the posts list.",
            Slot::RowSelected => "The selected row's shape.",
            Slot::EditorFrame => "Around the editor's text column (never behind the text).",
            Slot::QuoteFrame => "The frame of a quote box in the stream.",
            Slot::Divider => "The rule under the editor and between posts in the stream.",
            Slot::StatusOrnament => "A mark at the right of the status bar.",
            Slot::MarkerPinned => "The mark on a published post in the list.",
            Slot::MarkerNew => "The mark on a post with unpublished edits.",
            Slot::Texture => "A faint texture on the sidebar and the editor's margins.",
            Slot::ScrollEdge => "A fade at the bottom of the posts list.",
            Slot::EmptyArt => "A small picture above an empty list.",
        }
    }

    pub fn from_key(key: &str) -> Option<Slot> {
        Slot::ALL.into_iter().find(|s| s.key() == key)
    }
}

// ------------------------------------------------------------------ ornaments

/// The ornament vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    None,
    Gradient,
    Strata,
    Roots,
    LitCell,
    Lattice,
    Woodgrain,
    ShojiGrid,
    Kintsugi,
    Moss,
    Mist,
    LateriteSpeckle,
    StoneSpeckle,
    SaltGrain,
    GlyphDivider,
    BoxCorners,
    RhumbLines,
    CompassRose,
    Soundings,
    Seigaiha,
    Sashiko,
    IndigoDye,
    Horizon,
    TideLine,
    Contours,
    HarbourGlow,
    TideSparkline,
    ZLevel,
    Glyph,
    Dot,
    Arch,
    Cushion,
    Outline,
    DoubleRule,
    Buoy,
    InsetBar,
    Lantern,
    Rule,
    Line,
    Svg,
}

/// What an ornament accepts.
struct KindSpec {
    kind: Kind,
    name: &'static str,
    slots: &'static [Slot],
    colors: (usize, usize),
    /// Takes a quoted text argument (a glyph).
    text: bool,
    variants: &'static [&'static str],
    docs: &'static str,
}

use Slot as S;

const ANY: &[Slot] = &Slot::ALL;
const PICTURE: &[Slot] = &[
    S::TitlebarBand,
    S::SidebarGround,
    S::SidebarTop,
    S::EditorFrame,
    S::Divider,
    S::Texture,
    S::EmptyArt,
];

const fn k(
    kind: Kind,
    name: &'static str,
    slots: &'static [Slot],
    colors: (usize, usize),
    docs: &'static str,
) -> KindSpec {
    KindSpec {
        kind,
        name,
        slots,
        colors,
        text: false,
        variants: &[],
        docs,
    }
}

const KINDS: &[KindSpec] = &[
    k(Kind::None, "none", ANY, (0, 0), "Nothing (the plain look)."),
    k(
        Kind::Gradient,
        "gradient",
        &[S::TitlebarBand, S::SidebarGround],
        (2, 3),
        "A top-to-bottom wash through two or three colours.",
    ),
    k(
        Kind::Strata,
        "strata",
        &[S::SidebarGround],
        (2, 6),
        "A soil cross-section: a grass band, then earth strata.",
    ),
    k(
        Kind::Roots,
        "roots",
        &[S::SidebarTop, S::Divider],
        (1, 1),
        "Root hairlines hanging from the edge.",
    ),
    k(
        Kind::LitCell,
        "lit-cell",
        &[S::RowSelected, S::EmptyArt],
        (1, 2),
        "A lit burrow room: an arched cell with a warm glow.",
    ),
    KindSpec {
        variants: &["asanoha", "kumiko"],
        ..k(
            Kind::Lattice,
            "lattice",
            &[S::TitlebarBand, S::SidebarGround, S::EmptyArt],
            (1, 2),
            "A kumiko lattice; variant=asanoha adds the hemp-leaf star.",
        )
    },
    k(
        Kind::Woodgrain,
        "woodgrain",
        &[S::SidebarGround, S::Texture],
        (1, 1),
        "Fine vertical hinoki grain.",
    ),
    k(
        Kind::ShojiGrid,
        "shoji-grid",
        &[S::EditorFrame],
        (1, 1),
        "A shoji paper grid in the editor's margins.",
    ),
    k(
        Kind::Kintsugi,
        "kintsugi",
        &[S::Divider],
        (1, 1),
        "A gold repaired crack.",
    ),
    k(
        Kind::Moss,
        "moss",
        &[S::SidebarGround, S::EmptyArt],
        (1, 3),
        "Soft moss cushions.",
    ),
    k(
        Kind::Mist,
        "mist",
        &[S::ScrollEdge],
        (1, 1),
        "A mist that thickens toward the bottom.",
    ),
    k(
        Kind::LateriteSpeckle,
        "laterite-speckle",
        &[S::Texture],
        (1, 1),
        "Laterite pitting: rusty specks.",
    ),
    k(
        Kind::StoneSpeckle,
        "stone-speckle",
        &[S::Texture],
        (1, 1),
        "A rough stone floor.",
    ),
    k(
        Kind::SaltGrain,
        "salt-grain",
        &[S::Texture],
        (1, 1),
        "Weathered salt grain.",
    ),
    KindSpec {
        text: true,
        ..k(
            Kind::GlyphDivider,
            "glyph-divider",
            &[S::Divider],
            (1, 1),
            "A row of one glyph (default ≈, a water line).",
        )
    },
    k(
        Kind::BoxCorners,
        "box-corners",
        &[S::QuoteFrame],
        (1, 1),
        "Box-drawing corners ╔ ╝.",
    ),
    k(
        Kind::RhumbLines,
        "rhumb-lines",
        &[S::EditorFrame],
        (1, 2),
        "Rhumb lines from a compass rose, in the margin.",
    ),
    k(
        Kind::CompassRose,
        "compass-rose",
        &[S::EditorFrame, S::EmptyArt],
        (1, 2),
        "A compass rose.",
    ),
    k(
        Kind::Soundings,
        "soundings",
        &[S::Texture],
        (1, 1),
        "Scattered depth soundings, as on a chart.",
    ),
    k(
        Kind::Seigaiha,
        "seigaiha",
        &[S::SidebarGround, S::Divider, S::EmptyArt],
        (1, 3),
        "Seigaiha waves; on the sidebar, two more colours are the ground.",
    ),
    k(
        Kind::Sashiko,
        "sashiko",
        &[S::QuoteFrame, S::Divider],
        (1, 1),
        "A sashiko running stitch.",
    ),
    k(
        Kind::IndigoDye,
        "indigo-dye",
        &[S::TitlebarBand, S::SidebarGround],
        (2, 2),
        "An indigo dye gradient, darker at the edge.",
    ),
    k(
        Kind::Horizon,
        "horizon",
        &[S::TitlebarBand, S::Divider],
        (1, 1),
        "A horizon line that fades at both ends.",
    ),
    k(
        Kind::TideLine,
        "tide-line",
        &[S::Divider],
        (1, 2),
        "Two wavering tide lines.",
    ),
    k(
        Kind::Contours,
        "contours",
        &[S::SidebarGround, S::EmptyArt],
        (1, 3),
        "Sea contours with depth soundings; two more colours are the ground.",
    ),
    k(
        Kind::HarbourGlow,
        "harbour-glow",
        &[S::EditorFrame],
        (2, 2),
        "The editor as a lit harbour: warm paper in a night sea (sea, glow).",
    ),
    k(
        Kind::TideSparkline,
        "tide-sparkline",
        &[S::StatusOrnament],
        (1, 2),
        "A tide sparkline of your writing over the last two weeks.",
    ),
    k(
        Kind::ZLevel,
        "z-level",
        &[S::StatusOrnament],
        (1, 1),
        "The selected post's depth, as a z-level.",
    ),
    KindSpec {
        text: true,
        ..k(
            Kind::Glyph,
            "glyph",
            &[S::MarkerPinned, S::MarkerNew, S::EmptyArt],
            (0, 1),
            "One glyph, e.g. glyph(\"☼\").",
        )
    },
    k(
        Kind::Dot,
        "dot",
        &[S::MarkerPinned, S::MarkerNew],
        (0, 1),
        "A small dot.",
    ),
    k(
        Kind::Arch,
        "arch",
        &[S::QuoteFrame],
        (1, 1),
        "An arched room.",
    ),
    k(
        Kind::Cushion,
        "cushion",
        &[S::RowSelected, S::QuoteFrame],
        (1, 1),
        "A moss-cushion shape.",
    ),
    k(
        Kind::Outline,
        "outline",
        &[S::RowSelected, S::QuoteFrame],
        (1, 1),
        "A plain hairline box.",
    ),
    k(
        Kind::DoubleRule,
        "double-rule",
        &[S::QuoteFrame],
        (1, 1),
        "A chart's double rule.",
    ),
    k(
        Kind::Buoy,
        "buoy",
        &[S::QuoteFrame],
        (1, 1),
        "Squared, with a buoy-coloured top edge.",
    ),
    k(
        Kind::InsetBar,
        "inset-bar",
        &[S::RowSelected, S::QuoteFrame],
        (1, 1),
        "A bar down the left edge.",
    ),
    k(
        Kind::Lantern,
        "lantern",
        &[S::RowSelected],
        (1, 2),
        "A lantern outline with a warm point.",
    ),
    k(
        Kind::Rule,
        "rule",
        &[S::QuoteFrame],
        (0, 1),
        "A rule down the left edge (the plain quote box).",
    ),
    k(
        Kind::Line,
        "line",
        &[S::Divider],
        (0, 1),
        "A plain hairline.",
    ),
    k(
        Kind::Svg,
        "svg",
        PICTURE,
        (0, 1),
        "Your own SVG, as svg(path) or svg(path, #colour): drawn in one colour.",
    ),
];

fn kind_spec(name: &str) -> Option<&'static KindSpec> {
    KINDS.iter().find(|s| s.name == name)
}

impl Kind {
    pub fn name(self) -> &'static str {
        KINDS
            .iter()
            .find(|s| s.kind == self)
            .map(|s| s.name)
            .unwrap_or("none")
    }
}

/// The vocabulary for docs: `(name, slots, docs)`.
pub fn vocabulary() -> impl Iterator<Item = (&'static str, Vec<&'static str>, &'static str)> {
    KINDS
        .iter()
        .map(|s| (s.name, s.slots.iter().map(|s| s.key()).collect(), s.docs))
}

/// A user SVG, checked and loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SvgArt {
    pub path: PathBuf,
    pub bytes: Arc<[u8]>,
}

/// An ornament with its arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct Ornament {
    pub kind: Kind,
    pub colors: Vec<Rgba>,
    /// `opacity=` (0 to 1); `None` = the ornament's own default.
    pub opacity: Option<f32>,
    pub text: Option<String>,
    pub variant: Option<String>,
    pub svg: Option<SvgArt>,
}

impl Ornament {
    pub fn none() -> Ornament {
        Ornament {
            kind: Kind::None,
            colors: Vec::new(),
            opacity: None,
            text: None,
            variant: None,
            svg: None,
        }
    }
    pub fn is_none(&self) -> bool {
        self.kind == Kind::None
    }
    /// The `i`th colour, if given.
    pub fn color(&self, i: usize) -> Option<Rgba> {
        self.colors.get(i).copied()
    }
}

/// The biggest user SVG accepted.
pub const SVG_MAX_BYTES: u64 = 256 * 1024;
/// The most elements a user SVG may have.
pub const SVG_MAX_ELEMENTS: usize = 2_000;

/// Load a user SVG, refusing anything that could be slow or reach out:
/// too big, too many elements, entities, scripts, embedded or external
/// images, filters, and deep `<use>` chains.
pub fn load_svg(path: &Path) -> Result<Arc<[u8]>, String> {
    let meta = std::fs::metadata(path)
        .map_err(|e| format!("can't read {}: {e}", super::paths::tilde(path)))?;
    if !meta.is_file() {
        return Err(format!("{} isn't a file", super::paths::tilde(path)));
    }
    if meta.len() > SVG_MAX_BYTES {
        return Err(format!(
            "{} is {} KB; an ornament SVG must be under {} KB",
            super::paths::tilde(path),
            meta.len() / 1024,
            SVG_MAX_BYTES / 1024
        ));
    }
    let bytes = std::fs::read(path)
        .map_err(|e| format!("can't read {}: {e}", super::paths::tilde(path)))?;
    check_svg(&bytes).map_err(|e| format!("{}: {e}", super::paths::tilde(path)))?;
    Ok(bytes.into())
}

/// The structural checks of [`load_svg`], on bytes.
pub fn check_svg(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() as u64 > SVG_MAX_BYTES {
        return Err("too big".into());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "not UTF-8 text".to_string())?;
    let lower = text.to_ascii_lowercase();
    if !lower.contains("<svg") {
        return Err("not an SVG".into());
    }
    for (bad, why) in [
        ("<!entity", "entities aren't allowed"),
        ("<!doctype", "a DOCTYPE isn't allowed"),
        ("<script", "scripts aren't allowed"),
        ("<image", "embedded images aren't allowed"),
        ("<foreignobject", "foreignObject isn't allowed"),
        ("<filter", "filters aren't allowed (they're slow)"),
        ("href=\"http", "external links aren't allowed"),
        ("href='http", "external links aren't allowed"),
    ] {
        if lower.contains(bad) {
            return Err(why.into());
        }
    }
    let elements = text.matches('<').count();
    if elements > SVG_MAX_ELEMENTS {
        return Err(format!(
            "{elements} elements; an ornament SVG may have at most {SVG_MAX_ELEMENTS}"
        ));
    }
    if lower.matches("<use").count() > 64 {
        return Err("too many <use> elements".into());
    }
    Ok(())
}

/// Split `a(b, c d)` arguments: commas and spaces separate; double quotes
/// keep spaces and commas.
fn tokens(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars();
    let mut quoted = false;
    let mut had_quote = false;
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted => quoted = false,
            '"' => {
                quoted = true;
                had_quote = true;
            }
            '\\' if quoted => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            c if !quoted && (c == ',' || c.is_whitespace()) => {
                if !cur.is_empty() || had_quote {
                    out.push(std::mem::take(&mut cur));
                    had_quote = false;
                }
            }
            c => cur.push(c),
        }
    }
    if quoted {
        return Err("unterminated quote".into());
    }
    if !cur.is_empty() || had_quote {
        out.push(cur);
    }
    Ok(out)
}

/// Parse a slot value such as `roots(#d9b98a, opacity=0.55)` for `slot`.
/// `dir` resolves a relative `svg(…)` path.
pub fn parse_ornament(value: &str, slot: Slot, dir: &Path) -> Result<Ornament, String> {
    let v = value.trim();
    let (name, args) = match v.find('(') {
        Some(i) => {
            let inner = v[i + 1..]
                .strip_suffix(')')
                .ok_or_else(|| format!("`{v}` is missing its closing `)`"))?;
            (v[..i].trim(), inner)
        }
        None => (v, ""),
    };
    let name = name.to_ascii_lowercase();
    let spec = kind_spec(&name).ok_or_else(|| {
        let known: Vec<&str> = KINDS
            .iter()
            .filter(|k| k.slots.contains(&slot))
            .map(|k| k.name)
            .collect();
        format!(
            "unknown ornament `{name}` (for {} use one of: {})",
            slot.key(),
            known.join(", ")
        )
    })?;
    if !spec.slots.contains(&slot) {
        let fits: Vec<&str> = spec.slots.iter().map(|s| s.key()).collect();
        return Err(format!(
            "`{name}` can't go in {} (it fits {})",
            slot.key(),
            fits.join(", ")
        ));
    }
    let mut o = Ornament {
        kind: spec.kind,
        ..Ornament::none()
    };
    let mut toks = tokens(args)?.into_iter();
    if spec.kind == Kind::Svg {
        let p = toks
            .next()
            .filter(|p| !p.is_empty())
            .ok_or("svg needs a path, e.g. svg(~/.config/blygger/themes/art/mine.svg)")?;
        let path = resolve_path(&p, dir);
        let bytes = load_svg(&path)?;
        o.svg = Some(SvgArt { path, bytes });
    }
    for t in toks {
        if t.starts_with('#') {
            o.colors.push(parse_color(&t)?);
        } else if let Some((key, val)) = t.split_once('=') {
            match key.trim() {
                "opacity" => {
                    let n: f32 = val
                        .trim()
                        .parse()
                        .ok()
                        .filter(|n: &f32| n.is_finite() && (0.0..=1.0).contains(n))
                        .ok_or_else(|| format!("opacity `{val}` must be a number from 0 to 1"))?;
                    o.opacity = Some(n);
                }
                "variant" if !spec.variants.is_empty() => {
                    let w = val.trim().to_ascii_lowercase();
                    if !spec.variants.contains(&w.as_str()) {
                        return Err(format!(
                            "{name}: variant `{w}` must be one of {}",
                            spec.variants.join(", ")
                        ));
                    }
                    o.variant = Some(w);
                }
                other => return Err(format!("{name}: unknown argument `{other}`")),
            }
        } else if spec.text && o.text.is_none() {
            if t.chars().count() > 8 {
                return Err(format!("{name}: `{t}` is too long for a glyph"));
            }
            o.text = Some(t);
        } else {
            return Err(format!("{name}: unexpected `{t}`"));
        }
    }
    let (lo, hi) = spec.colors;
    let n = o.colors.len();
    if n < lo || n > hi {
        let want = match (lo, hi) {
            (0, 0) => "no colours".to_string(),
            (l, h) if l == h => format!("{l} colour{}", if l == 1 { "" } else { "s" }),
            (0, h) => format!("at most {h} colour{}", if h == 1 { "" } else { "s" }),
            (l, h) => format!("{l} to {h} colours"),
        };
        return Err(format!("`{name}` takes {want}, not {n}"));
    }
    if spec.kind == Kind::Glyph && o.text.is_none() {
        return Err("glyph needs a quoted glyph, e.g. glyph(\"☼\")".into());
    }
    Ok(o)
}

// ------------------------------------------------------------------ keys

#[derive(Debug, Clone, Copy, PartialEq)]
enum KeyKind {
    Name,
    Family,
    Base,
    Inherit,
    Color,
    Font,
    Radius,
    Border,
    Slot(Slot),
}

/// The non-colour, non-slot keys, for docs: `(key, docs)`.
pub const OTHER_KEYS: &[(&str, &str)] = &[
    (
        "name",
        "The name Settings shows. Defaults to the file name.",
    ),
    (
        "family",
        "plain, woody or oceanic: where Settings lists it.",
    ),
    (
        "base",
        "light or dark: what every unset key falls back to, and whether the app is dark.",
    ),
    (
        "inherit",
        "A built-in theme to start from; this file's keys override it.",
    ),
    (
        "font-writing",
        "The editor's font, unless font-family-writing is set in the config.",
    ),
    (
        "font-ui",
        "The list's and sheets' font, unless font-family-ui is set in the config.",
    ),
    (
        "font-chrome",
        "The status bar's and small labels' font (default Inter), unless font-family-ui is set.",
    ),
    (
        "radius",
        "Corner radius in points, 0 to 32 (inputs, rows, quote boxes).",
    ),
    (
        "sheet.radius",
        "A sheet's corner radius (default radius + 4).",
    ),
    ("toast.radius", "A toast's corner radius (default radius)."),
    ("chip.radius", "A chip's corner radius (default: a pill)."),
    ("sheet.border", "solid, dashed, double or none."),
    ("chip.border", "solid, dashed, double or none."),
    (
        "toast.border",
        "solid, dashed, double or none (default none).",
    ),
];

fn key_kind(key: &str) -> Option<KeyKind> {
    if let Some(c) = key.strip_prefix("color-") {
        return color_key(c).map(|_| KeyKind::Color);
    }
    if let Some(s) = Slot::from_key(key) {
        return Some(KeyKind::Slot(s));
    }
    Some(match key {
        "name" => KeyKind::Name,
        "family" => KeyKind::Family,
        "base" => KeyKind::Base,
        "inherit" => KeyKind::Inherit,
        "font-writing" | "font-ui" | "font-chrome" => KeyKind::Font,
        "radius" | "sheet.radius" | "toast.radius" | "chip.radius" => KeyKind::Radius,
        "sheet.border" | "chip.border" | "toast.border" => KeyKind::Border,
        _ => return None,
    })
}

fn all_keys() -> Vec<String> {
    let mut v: Vec<String> = OTHER_KEYS.iter().map(|(k, _)| k.to_string()).collect();
    v.extend(COLORS.iter().map(|c| format!("color-{}", c.name)));
    v.extend(Slot::ALL.iter().map(|s| s.key().to_string()));
    v
}

fn suggest(key: &str) -> Option<String> {
    all_keys()
        .into_iter()
        .map(|k| (super::parse::levenshtein(key, &k), k))
        .filter(|(d, _)| *d <= 3)
        .min_by_key(|(d, _)| *d)
        .map(|(_, k)| k)
}

// ------------------------------------------------------------------ files

/// One validated value.
#[derive(Debug, Clone, PartialEq)]
enum Value {
    Text(String),
    Family(Family),
    Base(Base),
    Color(Rgba),
    Radius(f32),
    Border(Border),
    Ornament(Ornament),
}

/// A parsed theme file: its valid lines, in order.
#[derive(Debug, Clone, PartialEq)]
pub struct ThemeFile {
    pub id: String,
    /// `None` for a built-in.
    pub path: Option<PathBuf>,
    values: Vec<(String, Value)>,
    /// Where each accepted key was last set.
    lines: Vec<(String, usize)>,
    /// The `inherit` line (for diagnostics).
    inherit_line: usize,
}

impl ThemeFile {
    fn get(&self, key: &str) -> Option<&Value> {
        self.values
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
    }
    fn inherit(&self) -> Option<&str> {
        match self.get("inherit") {
            Some(Value::Text(t)) => Some(t),
            _ => None,
        }
    }
}

/// Parse a theme file's text. `file` names it in diagnostics; `dir`
/// resolves relative `svg(…)` paths.
pub fn parse_theme(
    id: &str,
    text: &str,
    file: &Path,
    dir: &Path,
    builtin: bool,
) -> (ThemeFile, Vec<Diagnostic>) {
    let (entries, mut diags) = parse_text(text, file);
    let mut out = ThemeFile {
        id: id.to_string(),
        path: (!builtin).then(|| file.to_path_buf()),
        values: Vec::new(),
        lines: Vec::new(),
        inherit_line: 0,
    };
    let diag = |line: usize, severity: Severity, message: String| Diagnostic {
        file: file.to_path_buf(),
        line,
        severity,
        message,
    };
    for e in entries {
        let key = e.key.to_ascii_lowercase();
        let raw = e.value.trim();
        let Some(kind) = key_kind(&key) else {
            let hint = suggest(&key)
                .map(|s| format!(" (did you mean `{s}`?)"))
                .unwrap_or_default();
            diags.push(diag(
                e.line,
                Severity::Warning,
                format!("unknown theme key `{}`{hint}; ignored", e.key),
            ));
            continue;
        };
        if raw.is_empty() {
            // An empty value: back to what the layers below say.
            out.values.retain(|(k, _)| *k != key);
            continue;
        }
        let v: Result<Value, String> = match kind {
            KeyKind::Name | KeyKind::Font => Ok(Value::Text(raw.to_string())),
            KeyKind::Family => Family::parse(&raw.to_ascii_lowercase())
                .map(Value::Family)
                .ok_or_else(|| format!("`{raw}` must be plain, woody or oceanic")),
            KeyKind::Base => match raw.to_ascii_lowercase().as_str() {
                "light" => Ok(Value::Base(Base::Light)),
                "dark" => Ok(Value::Base(Base::Dark)),
                _ => Err(format!("`{raw}` must be light or dark")),
            },
            KeyKind::Inherit => {
                let n = raw.to_ascii_lowercase();
                if builtin_text(&n).is_some() {
                    out.inherit_line = e.line;
                    Ok(Value::Text(n))
                } else {
                    Err(format!(
                        "`{raw}` isn't a built-in theme (one of {})",
                        BUILTIN
                            .iter()
                            .map(|(n, _)| *n)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                }
            }
            KeyKind::Color => parse_color(raw).map(Value::Color),
            KeyKind::Radius => raw
                .parse::<f32>()
                .ok()
                .filter(|n| n.is_finite() && (0.0..=32.0).contains(n))
                .map(Value::Radius)
                .ok_or_else(|| format!("`{raw}` must be a number from 0 to 32")),
            KeyKind::Border => Border::parse(&raw.to_ascii_lowercase())
                .map(Value::Border)
                .ok_or_else(|| format!("`{raw}` must be solid, dashed, double or none")),
            KeyKind::Slot(slot) => parse_ornament(raw, slot, dir).map(Value::Ornament),
        };
        match v {
            Ok(v) => {
                out.lines.retain(|(k, _)| *k != key);
                out.lines.push((key.clone(), e.line));
                out.values.push((key, v));
            }
            Err(m) => diags.push(diag(e.line, Severity::Error, format!("{key}: {m}"))),
        }
    }
    (out, diags)
}

// ------------------------------------------------------------------ resolved

/// A theme with every value filled in.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub id: String,
    pub name: String,
    pub family: Family,
    pub base: Base,
    pub builtin: bool,
    colors: BTreeMap<&'static str, Rgba>,
    pub font_writing: Option<String>,
    pub font_ui: Option<String>,
    pub font_chrome: Option<String>,
    pub radius: f32,
    pub sheet_radius: f32,
    pub toast_radius: f32,
    pub chip_radius: f32,
    pub sheet_border: Border,
    pub chip_border: Border,
    pub toast_border: Border,
    slots: BTreeMap<Slot, Ornament>,
}

impl Resolved {
    /// A colour by its name without `color-` (every [`COLORS`] name resolves).
    pub fn color(&self, name: &str) -> Rgba {
        self.colors.get(name).copied().unwrap_or(Rgba(0xff00ffff))
    }
    pub fn slot(&self, slot: Slot) -> &Ornament {
        static NONE: std::sync::OnceLock<Ornament> = std::sync::OnceLock::new();
        self.slots
            .get(&slot)
            .unwrap_or_else(|| NONE.get_or_init(Ornament::none))
    }
    pub fn is_dark(&self) -> bool {
        self.base == Base::Dark
    }

    /// The same look under another id and name (compare a copy with its
    /// original).
    pub fn with_identity(mut self, id: &str, name: &str, builtin: bool) -> Resolved {
        self.id = id.to_string();
        self.name = name.to_string();
        self.builtin = builtin;
        self
    }
}

fn resolve_layers(id: &str, layers: &[&ThemeFile], builtin: bool) -> Resolved {
    let get = |key: &str| layers.iter().rev().find_map(|l| l.get(key));
    let base = match get("base") {
        Some(Value::Base(b)) => *b,
        _ => Base::Light,
    };
    let mut colors: BTreeMap<&'static str, Rgba> = BTreeMap::new();
    for c in COLORS {
        let v = match get(&format!("color-{}", c.name)) {
            Some(Value::Color(v)) => Some(*v),
            _ => match c.fallback {
                Fallback::Required => None,
                Fallback::Same(o) => colors.get(o).copied(),
                Fallback::Alpha(o, a) => colors.get(o).map(|c| c.with_alpha(a)),
            },
        };
        if let Some(v) = v {
            colors.insert(c.name, v);
        }
    }
    let text = |key: &str| match get(key) {
        Some(Value::Text(t)) => Some(t.clone()),
        _ => None,
    };
    let radius_of = |key: &str| match get(key) {
        Some(Value::Radius(r)) => Some(*r),
        _ => None,
    };
    let border_of = |key: &str, d: Border| match get(key) {
        Some(Value::Border(b)) => *b,
        _ => d,
    };
    let radius = radius_of("radius").unwrap_or(8.0);
    let mut slots = BTreeMap::new();
    for s in Slot::ALL {
        if let Some(Value::Ornament(o)) = get(s.key())
            && !o.is_none()
        {
            slots.insert(s, o.clone());
        }
    }
    Resolved {
        id: id.to_string(),
        name: text("name").unwrap_or_else(|| id.to_string()),
        family: match get("family") {
            Some(Value::Family(f)) => *f,
            _ => Family::Plain,
        },
        base,
        builtin,
        colors,
        font_writing: text("font-writing"),
        font_ui: text("font-ui"),
        font_chrome: text("font-chrome"),
        radius,
        sheet_radius: radius_of("sheet.radius").unwrap_or(radius + 4.0),
        toast_radius: radius_of("toast.radius").unwrap_or(radius),
        chip_radius: radius_of("chip.radius").unwrap_or(99.0),
        sheet_border: border_of("sheet.border", Border::Solid),
        chip_border: border_of("chip.border", Border::Solid),
        toast_border: border_of("toast.border", Border::None),
        slots,
    }
}

/// The built-in themes plus the user's, parsed once.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    builtins: Vec<ThemeFile>,
    user: Vec<ThemeFile>,
    pub diagnostics: Vec<Diagnostic>,
    /// Where user themes are read from (`None`: built-ins only).
    pub dir: Option<PathBuf>,
}

/// The largest theme file read.
const MAX_THEME_BYTES: u64 = 64 * 1024;
/// The most user themes read.
const MAX_THEMES: usize = 200;

impl Registry {
    /// Only the built-in themes.
    pub fn builtin() -> Registry {
        let mut r = Registry::default();
        for (id, text) in BUILTIN {
            let file = PathBuf::from(format!("<built-in theme {id}>"));
            let (t, d) = parse_theme(id, text, &file, Path::new("."), true);
            debug_assert!(d.is_empty(), "built-in {id}: {d:?}");
            r.diagnostics.extend(d);
            r.builtins.push(t);
        }
        r
    }

    /// Built-ins plus every theme file in `dir` (a missing dir is fine).
    /// A user theme with a built-in's name replaces it.
    pub fn load(dir: &Path) -> Registry {
        let mut r = Registry::builtin();
        r.dir = Some(dir.to_path_buf());
        let Ok(read) = std::fs::read_dir(dir) else {
            return r;
        };
        let mut files: Vec<PathBuf> = read
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect();
        files.sort();
        for path in files.into_iter().take(MAX_THEMES) {
            let Some(stem) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            // Editors' backups and hidden files aren't themes.
            if stem.starts_with('.') || stem.ends_with('~') || stem.ends_with(".swp") {
                continue;
            }
            let id = stem.strip_suffix(".theme").unwrap_or(stem).to_string();
            let diag = |message: String| Diagnostic {
                file: path.clone(),
                line: 0,
                severity: Severity::Warning,
                message,
            };
            if !valid_name(&id) {
                r.diagnostics.push(diag(format!(
                    "`{id}` isn't a theme name (use lowercase letters, digits and -); skipped"
                )));
                continue;
            }
            if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_THEME_BYTES) {
                r.diagnostics
                    .push(diag("too big for a theme file; skipped".into()));
                continue;
            }
            match std::fs::read_to_string(&path) {
                Ok(text) => r.add_user(&id, &text, &path),
                Err(e) => r.diagnostics.push(diag(format!("couldn't read it: {e}"))),
            }
        }
        r
    }

    /// Add a user theme from text (tests, and [`Registry::load`]).
    pub fn add_user(&mut self, id: &str, text: &str, path: &Path) {
        let dir = path.parent().unwrap_or(Path::new("."));
        let (t, d) = parse_theme(id, text, path, dir, false);
        self.diagnostics.extend(d);
        self.user.retain(|u| u.id != id);
        self.user.push(t);
    }

    fn find(&self, id: &str) -> Option<&ThemeFile> {
        self.user
            .iter()
            .find(|t| t.id == id)
            .or_else(|| self.builtins.iter().find(|t| t.id == id))
    }

    fn builtin_file(&self, id: &str) -> Option<&ThemeFile> {
        self.builtins.iter().find(|t| t.id == id)
    }

    pub fn exists(&self, id: &str) -> bool {
        id == SYSTEM || self.find(id).is_some()
    }

    /// Every theme id, built-ins first (in their order), then the user's.
    pub fn ids(&self) -> Vec<String> {
        let mut v: Vec<String> = self.builtins.iter().map(|t| t.id.clone()).collect();
        for u in &self.user {
            if !v.contains(&u.id) {
                v.push(u.id.clone());
            }
        }
        v
    }

    /// Resolve a theme by id (not `system`).
    pub fn resolve(&self, id: &str) -> Option<Resolved> {
        let top = self.find(id)?;
        let builtin = top.path.is_none();
        let mut layers: Vec<&ThemeFile> = vec![top];
        // `inherit` names a built-in, so the chain is short and acyclic; a
        // built-in that inherits (none do today) is followed at most 4 deep.
        let mut next = top.inherit().map(str::to_string);
        while let Some(parent) = next.take() {
            if layers.len() > 4 || layers.iter().any(|l| l.path.is_none() && l.id == parent) {
                break;
            }
            if let Some(p) = self.builtin_file(&parent) {
                next = p.inherit().map(str::to_string);
                layers.push(p);
            }
        }
        let base = layers
            .iter()
            .find_map(|l| match l.get("base") {
                Some(Value::Base(b)) => Some(*b),
                _ => None,
            })
            .unwrap_or_default();
        if let Some(b) = self.builtin_file(base.id())
            && !layers.iter().any(|l| std::ptr::eq(*l, b))
        {
            layers.push(b);
        }
        layers.reverse();
        Some(resolve_layers(id, &layers, builtin))
    }

    /// The theme to use: `theme` (maybe `system`), with `theme-dark` when
    /// macOS is dark. Unknown names fall back to the plain light/dark.
    pub fn pick(&self, theme: &str, theme_dark: Option<&str>, os_dark: bool) -> Resolved {
        let want = self.pick_id(theme, theme_dark, os_dark);
        self.resolve(want)
            .or_else(|| self.resolve("light"))
            .unwrap_or_else(|| resolve_layers("light", &[], true))
    }

    /// The id [`Registry::pick`] resolves.
    pub fn pick_id<'a>(
        &self,
        theme: &'a str,
        theme_dark: Option<&'a str>,
        os_dark: bool,
    ) -> &'a str {
        match (os_dark, theme_dark) {
            (true, Some(d)) if self.find(d).is_some() => d,
            _ if theme == SYSTEM || self.find(theme).is_none() => {
                if os_dark {
                    "dark"
                } else {
                    "light"
                }
            }
            _ => theme,
        }
    }

    /// Every user theme's font lines, `(file, line, key, value)`, for the
    /// app to check against the fonts it has.
    pub fn user_fonts(&self) -> Vec<(PathBuf, usize, String, String)> {
        let mut out = Vec::new();
        for t in &self.user {
            let Some(path) = &t.path else { continue };
            for (key, line) in &t.lines {
                if key.starts_with("font-")
                    && let Some(Value::Text(v)) = t.get(key)
                {
                    out.push((path.clone(), *line, key.clone(), v.clone()));
                }
            }
        }
        out
    }

    /// Where the `inherit` line of a user theme is, for messages.
    pub fn inherit_line(&self, id: &str) -> Option<usize> {
        self.find(id).map(|t| t.inherit_line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(text: &str) -> (Registry, Vec<Diagnostic>) {
        let mut r = Registry::builtin();
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        r.add_user("mine", text, Path::new("/tmp/themes/mine"));
        let d = r.diagnostics.clone();
        (r, d)
    }

    #[test]
    fn every_builtin_parses_cleanly_and_is_complete() {
        let r = Registry::builtin();
        assert!(r.diagnostics.is_empty(), "{:#?}", r.diagnostics);
        assert_eq!(r.ids().len(), BUILTIN.len());
        for (id, _) in BUILTIN {
            assert!(valid_name(id));
            let t = r.resolve(id).unwrap();
            for c in COLORS {
                assert!(
                    t.colors.contains_key(c.name),
                    "{id}: color-{} unset",
                    c.name
                );
            }
        }
        let fam = |id: &str| r.resolve(id).unwrap().family;
        assert_eq!(fam("light"), Family::Plain);
        assert_eq!(fam("cutaway"), Family::Woody);
        assert_eq!(fam("konkan"), Family::Oceanic);
        assert!(r.resolve("fortress").unwrap().is_dark());
        assert_eq!(r.resolve("konkan").unwrap().name, "Konkan harbour");
    }

    #[test]
    fn body_text_contrast_is_at_least_4_5() {
        let r = Registry::builtin();
        for (id, _) in BUILTIN {
            let t = r.resolve(id).unwrap();
            for (fg, bg, min) in [
                ("ink", "bg", 4.5),
                ("ink", "editor", 4.5),
                ("ink", "quote-bg", 4.5),
                ("side-ink", "side", 4.5),
                ("sel-ink", "sel", 4.5),
                ("toast-ink", "toast", 4.5),
                ("muted", "bg", 3.0),
                ("side-muted", "side", 3.0),
                ("bar-ink", "bar", 3.0),
                ("status-ink", "status", 3.0),
            ] {
                let c = t.color(fg).contrast(t.color(bg));
                assert!(c >= min, "{id}: {fg} on {bg} is {c:.2}:1 (< {min})");
            }
        }
    }

    #[test]
    fn colours_parse() {
        assert_eq!(parse_color("#abc").unwrap(), Rgba(0xaabbccff));
        assert_eq!(parse_color("#abcd").unwrap(), Rgba(0xaabbccdd));
        assert_eq!(parse_color("#a4271b").unwrap(), Rgba(0xa4271bff));
        assert_eq!(parse_color("#a4271b33").unwrap(), Rgba(0xa4271b33));
        assert!(parse_color("a4271b").is_err());
        assert!(parse_color("#a4271").is_err());
        assert!(parse_color("#gggggg").is_err());
        assert_eq!(Rgba(0xa4271b33).hex(), "#a4271b33");
        assert_eq!(Rgba(0xa4271bff).hex(), "#a4271b");
        assert!((Rgba(0x000000ff).contrast(Rgba(0xffffffff)) - 21.0).abs() < 0.01);
    }

    #[test]
    fn a_good_file_resolves_over_its_base() {
        let (r, d) = user(
            "name = Mine\nfamily = oceanic\nbase = dark\ncolor-bg = #102030\n\
             radius = 4\nfont-writing = Charter\ndivider = tide-line(#b8c0c0)\n",
        );
        assert!(d.is_empty(), "{d:?}");
        let t = r.resolve("mine").unwrap();
        assert_eq!(t.name, "Mine");
        assert_eq!(t.family, Family::Oceanic);
        assert!(t.is_dark());
        assert_eq!(t.color("bg"), Rgba(0x102030ff));
        // Unset: from the dark base; derived: from bg.
        assert_eq!(t.color("ink"), Rgba(0xe4dfd3ff));
        assert_eq!(t.color("side"), Rgba(0x102030ff));
        assert_eq!(t.color("editor"), Rgba(0x102030ff));
        assert_eq!(t.radius, 4.0);
        assert_eq!(t.sheet_radius, 8.0);
        assert_eq!(t.font_writing.as_deref(), Some("Charter"));
        assert_eq!(t.slot(Slot::Divider).kind, Kind::TideLine);
        assert!(!t.builtin);
    }

    #[test]
    fn inherit_starts_from_a_builtin() {
        let (r, d) = user("inherit = cutaway\ncolor-accent = #123456\nsidebar.top = none\n");
        assert!(d.is_empty(), "{d:?}");
        let t = r.resolve("mine").unwrap();
        let cut = r.resolve("cutaway").unwrap();
        assert_eq!(t.color("accent"), Rgba(0x123456ff));
        assert_eq!(t.color("side"), cut.color("side"));
        assert_eq!(t.family, Family::Woody);
        assert_eq!(t.name, "Cutaway", "the name is inherited too");
        assert_eq!(t.slot(Slot::SidebarGround), cut.slot(Slot::SidebarGround));
        assert!(
            t.slot(Slot::SidebarTop).is_none(),
            "none switches a slot off"
        );
        let (_, d) = user("inherit = nope\n");
        assert_eq!(d.len(), 1);
        assert!(d[0].message.contains("isn't a built-in theme"), "{d:?}");
    }

    #[test]
    fn unknown_keys_and_bad_values_are_reported_with_lines() {
        let (r, d) = user(
            "color-bg = #fff\ncolour-ink = #000\ncolor-ink = black\nradius = 99\n\
             sidebar.ground = kintsugi(#c9a23a)\ndivider = sparkles\nbase = dusk\n\
             sidebar.top = roots(#d9b98a, opacity=2)\nrow.selected = lit-cell\nnot a line\n",
        );
        let got: Vec<(usize, Severity)> = d.iter().map(|d| (d.line, d.severity)).collect();
        assert_eq!(
            got,
            vec![
                (10, Severity::Error),
                (2, Severity::Warning),
                (3, Severity::Error),
                (4, Severity::Error),
                (5, Severity::Error),
                (6, Severity::Error),
                (7, Severity::Error),
                (8, Severity::Error),
                (9, Severity::Error),
            ],
            "{d:#?}"
        );
        assert!(
            d[1].message.contains("did you mean `color-ink`"),
            "{}",
            d[1].message
        );
        assert!(d[3].message.contains("0 to 32"));
        assert!(
            d[4].message.contains("can't go in sidebar.ground"),
            "{}",
            d[4].message
        );
        assert!(d[5].message.contains("unknown ornament `sparkles`"));
        assert!(
            d[8].message.contains("takes 1 to 2 colours"),
            "{}",
            d[8].message
        );
        // The good line still applies; the bad ones don't.
        let t = r.resolve("mine").unwrap();
        assert_eq!(t.color("bg"), Rgba(0xffffffff));
        assert_eq!(t.color("ink"), Rgba(0x111111ff));
        assert_eq!(t.radius, 8.0);
    }

    #[test]
    fn ornament_arguments() {
        let d = Path::new("/");
        let o = parse_ornament("roots(#d9b98a, opacity=0.55)", Slot::SidebarTop, d).unwrap();
        assert_eq!(o.colors, vec![Rgba(0xd9b98aff)]);
        assert_eq!(o.opacity, Some(0.55));
        let o = parse_ornament("strata(#7a8f45 #5b3d25)", Slot::SidebarGround, d).unwrap();
        assert_eq!(o.colors.len(), 2);
        let o = parse_ornament("glyph-divider(#4f7fa8, \"≈ ~\")", Slot::Divider, d).unwrap();
        assert_eq!(o.text.as_deref(), Some("≈ ~"));
        let o = parse_ornament("lattice(#785a32, variant=asanoha)", Slot::TitlebarBand, d);
        assert_eq!(o.unwrap().variant.as_deref(), Some("asanoha"));
        assert!(parse_ornament("lattice(#785a32, variant=hex)", Slot::TitlebarBand, d).is_err());
        assert!(parse_ornament("roots(#d9b98a", Slot::SidebarTop, d).is_err());
        assert!(parse_ornament("roots(#d9b98a, size=3)", Slot::SidebarTop, d).is_err());
        assert!(parse_ornament("glyph", Slot::MarkerNew, d).is_err());
        assert!(parse_ornament("none", Slot::EmptyArt, d).unwrap().is_none());
        assert_eq!(
            parse_ornament("  Kintsugi( #c9a23a ) ", Slot::Divider, d)
                .unwrap()
                .kind,
            Kind::Kintsugi
        );
    }

    #[test]
    fn svg_paths_resolve_and_are_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let art = dir.path().join("art");
        std::fs::create_dir_all(&art).unwrap();
        std::fs::write(
            art.join("ok.svg"),
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><circle r="4" cx="5" cy="5"/></svg>"#,
        )
        .unwrap();
        std::fs::write(art.join("script.svg"), "<svg><script>x</script></svg>").unwrap();
        std::fs::write(
            art.join("lol.svg"),
            "<!DOCTYPE svg [<!ENTITY a \"aaaa\">]><svg>&a;</svg>",
        )
        .unwrap();
        let many = format!("<svg>{}</svg>", "<g/>".repeat(SVG_MAX_ELEMENTS + 1));
        std::fs::write(art.join("many.svg"), many).unwrap();
        std::fs::write(art.join("big.svg"), vec![b' '; SVG_MAX_BYTES as usize + 1]).unwrap();

        let o = parse_ornament("svg(art/ok.svg, #123456)", Slot::EmptyArt, dir.path()).unwrap();
        let svg = o.svg.unwrap();
        assert_eq!(svg.path, art.join("ok.svg"));
        assert!(!svg.bytes.is_empty());
        assert_eq!(o.colors, vec![Rgba(0x123456ff)]);
        for (f, why) in [
            ("script.svg", "scripts"),
            ("lol.svg", "entities"),
            ("many.svg", "elements"),
            ("big.svg", "must be under"),
            ("missing.svg", "can't read"),
        ] {
            let e =
                parse_ornament(&format!("svg(art/{f})"), Slot::EmptyArt, dir.path()).unwrap_err();
            assert!(e.contains(why), "{f}: {e}");
        }
        assert!(parse_ornament("svg()", Slot::EmptyArt, dir.path()).is_err());
        assert!(
            parse_ornament("svg(art/ok.svg)", Slot::MarkerNew, dir.path())
                .unwrap_err()
                .contains("can't go in marker.new")
        );
        // `~/` is the home directory.
        let home = std::env::var("HOME").unwrap_or_default();
        let e =
            parse_ornament("svg(~/no-such-dir-xyz/a.svg)", Slot::EmptyArt, dir.path()).unwrap_err();
        assert!(!home.is_empty() && e.contains("no-such-dir-xyz"), "{e}");
    }

    #[test]
    fn load_reads_a_directory_and_user_themes_shadow_builtins() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("cutaway"), builtin_text("cutaway").unwrap()).unwrap();
        std::fs::write(
            dir.path().join("sea-glass"),
            "inherit = saltspace\ncolor-accent = #2a9d8f\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("Bad Name"), "base = light\n").unwrap();
        std::fs::write(dir.path().join(".hidden"), "junk\n").unwrap();
        let r = Registry::load(dir.path());
        assert_eq!(r.diagnostics.len(), 1, "{:?}", r.diagnostics);
        assert!(r.diagnostics[0].message.contains("isn't a theme name"));
        assert!(r.exists("sea-glass"));
        assert!(r.ids().ends_with(&["sea-glass".to_string()]));
        let t = r.resolve("cutaway").unwrap();
        assert!(!t.builtin, "the user's copy wins");
        let b = Registry::builtin().resolve("cutaway").unwrap();
        assert_eq!(Resolved { builtin: true, ..t }, b, "a copy round-trips");
        // A missing dir is just the built-ins.
        let r = Registry::load(&dir.path().join("nope"));
        assert!(r.diagnostics.is_empty());
        assert_eq!(r.ids().len(), BUILTIN.len());
    }

    #[test]
    fn pick_follows_the_system_and_theme_dark() {
        let r = Registry::builtin();
        assert_eq!(r.pick("system", None, false).id, "light");
        assert_eq!(r.pick("system", None, true).id, "dark");
        assert_eq!(r.pick("cutaway", None, true).id, "cutaway");
        assert_eq!(r.pick("cutaway", Some("fortress"), true).id, "fortress");
        assert_eq!(r.pick("cutaway", Some("fortress"), false).id, "cutaway");
        assert_eq!(r.pick("system", Some("fortress"), true).id, "fortress");
        assert_eq!(r.pick("nope", None, false).id, "light");
        assert_eq!(r.pick("nope", Some("nope"), true).id, "dark");
        assert_eq!(r.pick("light", None, true).id, "light", "light stays light");
    }

    #[test]
    fn names() {
        assert!(valid_name("konkan"));
        assert!(valid_name("my-theme-2"));
        assert!(!valid_name("My theme"));
        assert!(!valid_name("-x"));
        assert!(!valid_name("../x"));
        assert!(!valid_name(""));
    }
}
