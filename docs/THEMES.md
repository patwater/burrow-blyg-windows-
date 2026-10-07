# Themes

The app ships ten themes: the two plain looks, four **woody** ones (place:
warm, enclosed, textured) and four **oceanic** ones (space: horizon, charts,
salt). A theme is a plain-text file in the config file's `key = value`
syntax, so you can copy one, edit it and share it.

Themes change the app around your writing: the window, the lists, the editor
and the reader's fallback style. The studio preview is never themed. It shows
your blyg's own `style.css`, which is what readers see.

The Reading screens take the theme too. In the reader, a post's text is set
in the theme's writing font (yours, if you set `font-family-writing`), and
its quote boxes, selected text, highlights and diff marks take the theme's
colours (`color-quote-bg`, `color-quote-rule`, `color-text-selection`,
`color-ins`, `color-del`, `color-accent`). Small labels (bylines, dates,
chips) use the theme's `font-chrome`, as the posts list does.

## Choosing a theme

In Settings (⌘,), under Theme, the themes are grouped into Plain, Woody and
Oceanic, each with a small swatch. Clicking one writes `theme = <name>` to the
config file.

In the config file (`~/.config/blygger/config`):

```
theme = cutaway
theme-dark = fortress
```

| Key | Values |
| --- | --- |
| `theme` | `system` (the default: follows macOS light and dark), `light` (Paper), `dark` (Ink), a built-in theme, or the name of a file in `~/.config/blygger/themes/` |
| `theme-dark` | Optional. The theme to use while macOS is dark, whatever `theme` says. With it unset, `theme = system` switches to `dark` and every other theme stays as it is. |

`blygger +list-themes` lists every theme, built in and yours. An unknown name
shows in the config problems banner (with its line number), and the app falls
back to `system`.

For screenshots and development, `BLYGGER_THEME=<name>` overrides the theme
for one run without saving it.

## The built-in themes

| Name | Family | Ornaments |
| --- | --- | --- |
| `light` (Paper) | plain | none: the original cream-paper look |
| `dark` (Ink) | plain | none: the original dark look |
| `cutaway` | woody | a meadow-green title bar; a soil cross-section behind the posts list (a grass band, then earth strata with pebbles) with root hairlines along its top; the selected post is a lit burrow room with a warm glow; arched quote boxes; a root line under the editor |
| `kumiko` | woody | an asanoha lattice in the title bar; hinoki woodgrain on the posts list; a shoji grid in the editor's margins; hairline-boxed rows and quotes; a gold kintsugi crack under the editor; Source Serif 4 for writing |
| `shola` | woody | soft moss cushions on a forest-green posts list, with mist at its foot; moss-cushion rows and quotes; laterite pitting in the editor's margins; a laterite status bar |
| `fortress` | woody | a dark stone floor; `☼` on published posts and `*` on unpublished edits; a `≈≈≈` water line under the editor; box-drawing corners `╔ ╝` on quotes; the selected post's depth as a z-level in the status bar; Menlo for the interface, Literata for writing |
| `portolan` | oceanic | a chart's paper; compass roses with rhumb lines and depth soundings in the editor's margins; double-ruled quotes and sheets; ET Book throughout |
| `aizome` | oceanic | an indigo-dyed title bar and status bar; seigaiha waves on the posts list and under the editor; sashiko-stitched quotes |
| `saltspace` | oceanic | corrosive greys, never pure black or white; a horizon line through the title bar; salt grain on the posts list and the editor's margins; tide lines under the editor; squared quotes with a buoy-orange top edge |
| `konkan` (Konkan harbour) | oceanic | a night sea with contour lines and soundings behind the posts list; the editor as a lit harbour, warm paper glowing in the sea; lantern rows; a tide sparkline of your last two weeks' writing in the status bar |

Screenshots of each are in `docs/screenshots/themes/`.

## Fonts

A theme can suggest fonts (`font-writing`, `font-ui`, `font-chrome`). They are
used only when your config doesn't set `font-family-writing` or
`font-family-ui`: your settings always win. Picking a font in Settings sets it
in the config, and from then on it wins over every theme.

Themes can use the fonts the app bundles (Literata, Source Serif 4, Inter,
iA Writer Quattro, ET Book) or the ones macOS has (New York, Charter, Menlo,
SF Pro). `blygger +list-fonts` lists them.

## Make your own theme

1. Copy a built-in theme into your themes folder:

   ```
   blygger +copy-theme saltspace sea-glass
   ```

   This writes `~/.config/blygger/themes/sea-glass`. Without a new name
   (`blygger +copy-theme saltspace`), the copy keeps the built-in's name and
   replaces it. `+copy-theme` never overwrites an existing file.

2. Point the config at it: `theme = sea-glass`.

3. Edit the file and save it. The app watches the config files and the themes
   folder, and reloads within a second. ⌘⇧, reloads by hand. Problems show in
   the banner at the top of the window, with the file and line.

A theme doesn't have to be a full copy. This file is a complete theme:

```
# ~/.config/blygger/themes/sea-glass
name = Sea glass
inherit = saltspace
color-accent = #2a9d8f
quote.frame = buoy(#2a9d8f)
divider = tide-line(#9fc4bd #cfe3df)
```

Everything it doesn't set comes from `saltspace`.

## The file format

One `key = value` per line, and `#` starts a comment at the beginning of a
line (so `#fbf3e2` in a value is fine). A theme's name is its file name:
lowercase letters, digits and `-`. An empty value (`texture =`) resets a key
to what the layers below say.

A theme is resolved in layers, each overriding the one before:

1. the plain theme for its `base` (`light` or `dark`),
2. the built-in named by `inherit`, if any,
3. the file itself.

Colours a theme leaves unset are derived from others. For example, the
sidebar takes the background, and selected text takes the accent at 20%
opacity.

### Identity, shape and fonts

| Key | Meaning |
| --- | --- |
| `name` | The name Settings shows. Defaults to the file name. |
| `family` | `plain`, `woody` or `oceanic`: where Settings lists it. |
| `base` | `light` or `dark`: what unset keys fall back to, and whether the app is dark (for the reader and the browser pane). |
| `inherit` | A built-in theme to start from. |
| `font-writing` | The editor's font. |
| `font-ui` | The lists' and sheets' font. |
| `font-chrome` | The status bar's and small labels' font (default Inter). |
| `radius` | Corner radius in points, 0 to 32 (rows, quote boxes). Default 8. Small corners (buttons, fields, banners, the Stream/Reader toggle) scale with it: square on a chart, soft on moss. |
| `sheet.radius`, `toast.radius`, `chip.radius` | Those corners. Defaults: radius + 4, radius, and a pill. Chips include the reading screens' action chips ("Quote into a thread", "Reply · new stub", …); sheets include the reading sheets (Subscribe, New Folder, the quote picker, Open profile). |
| `sheet.border`, `chip.border`, `toast.border` | `solid`, `dashed`, `double` or `none`. |

### Colours

Colours are `#rgb`, `#rrggbb` or `#rrggbbaa`.

| Key | What it colours | Unset: same as |
| --- | --- | --- |
| `color-bg` | the window, sheets, the reader | (base) |
| `color-ink` | body text | (base) |
| `color-muted` | dates, hints, labels | (base) |
| `color-line` | hairlines and borders | (base) |
| `color-accent` | links, the caret, highlights | (base) |
| `color-sel` | the selected row | (base) |

`color-sel` is picked for the sidebar. Lists on the page (the Reader's posts, the stream, Subscriptions, versions, profiles) use it too when page text reads on it and it stands apart from the page; otherwise (Konkan's dark lantern row, Aizome's row the colour of the page) they use a tint of the page toward it, or toward the sidebar.
| `color-bar` | the title bar | (base) |
| `color-warn`, `color-over`, `color-green`, `color-amber`, `color-grey` | the length counter and the sync dot | (base) |
| `color-shadow` | sheet and toast shadows | (base) |
| `color-edited`, `color-edited-bg` | the "edited" badge in the reading list | (base) |
| `color-notice`, `color-notice-bg` | notices in the reader (a withdrawn post) | (base) |
| `color-edited-rule` | the rule beside an author's edit notes | `edited` |
| `color-editor` | the editor's paper | `bg` |
| `color-text-selection` | selected text | `accent` at 20% |
| `color-ins`, `color-del` | inserted and deleted text in a diff | `green`, `over`, translucent |
| `color-divider` | dividers between panes | `line` |
| `color-side`, `color-side-ink`, `color-side-muted`, `color-side-accent`, `color-side-line` | the posts list, its text, marks and edge | `bg`, `ink`, `muted`, `accent`, `divider` |
| `color-sel-ink`, `color-sel-muted` | text in the selected row | `side-ink`, `side-muted` |
| `color-bar-ink`, `color-bar-line` | the window title, the title bar's edge | `muted`, `divider` |
| `color-status`, `color-status-ink`, `color-status-line` | the status bar | `bar`, `muted`, `divider` |
| `color-quote-bg`, `color-quote-rule` | a quote box | `sel`, `accent` |
| `color-toast`, `color-toast-ink` | a toast | `ink`, `bg` |

When the title bar or status bar is dark, the buttons and tabs on it take
their colour from `color-bar-ink` and `color-status-ink`.

Sheets, menus, popups, fields and chips are drawn from the same colours, and
Burrow adjusts them where a theme's choice wouldn't read there: an accent,
warning or error colour used as text is darkened (or lightened) until it
reaches 4.5:1 on its ground, status bar text and links against
`color-status` too; a menu's chosen row uses `color-sel` only when body text
reads on it (a dark selected row, meant for a dark sidebar, becomes a tint of
the page there); borders of fields and popups are kept visible on dark
grounds; and the veil behind a setup card always darkens. Tooltips take
`color-toast` and `color-toast-ink`.

### Ornament slots

A slot takes one ornament from the vocabulary below, optionally with colours
and arguments: `name`, or `name(#colour #colour, opacity=0.5)`. Commas and
spaces both separate arguments, and double quotes keep a glyph together.

| Slot | Where |
| --- | --- |
| `titlebar.band` | behind the title bar |
| `sidebar.ground` | behind the posts list and the Reader's subscriptions column |
| `sidebar.top` | along the top of the posts list and the Reader's subscriptions column |
| `row.selected` | the selected row's shape: the posts list, and on the reading side the subscriptions column and the posts list (in the pane with the keys), the stream's selected post, Subscriptions and a profile's lists |
| `editor.frame` | around the editor's text column, in its margins only |
| `quote.frame` | a quote box in the stream |
| `divider` | the band under the editor and between posts in the stream |
| `status.ornament` | a mark at the right of the status bar |
| `marker.pinned` | the mark after a published post's version, and beside a pinned version in ⌘Y |
| `marker.new` | the mark on a post with unpublished edits, an unread post in the stream, an unread subscription and a new mention |
| `texture` | a faint texture on the posts list, the Reader's subscriptions column and the editor's margins |
| `scroll.edge` | a fade at the bottom of the posts list and the Reader's subscriptions column |
| `empty.art` | a small picture above an empty posts list, and above the reading side's empty states (nothing to read, all read, no subscriptions, no mentions) |

Ornaments never sit behind the text you're writing: the editor's ornaments
are clipped to its margins, and the divider takes its own row under the text.

### The vocabulary

| Ornament | Slots | Colours | Notes |
| --- | --- | --- | --- |
| `none` | any | none | the plain look |
| `gradient` | titlebar.band, sidebar.ground | 2 to 3 | top to bottom; three colours run across |
| `strata` | sidebar.ground | 2 to 6 | grass, then earth strata |
| `roots` | sidebar.top, divider | 1 | root hairlines |
| `lit-cell` | row.selected, empty.art | 1 to 2 | a lit burrow room |
| `lattice` | titlebar.band, sidebar.ground, empty.art | 1 to 2 | `variant=asanoha` (default) or `variant=kumiko` |
| `woodgrain` | sidebar.ground, texture | 1 | fine vertical grain |
| `shoji-grid` | editor.frame | 1 | a shoji paper grid |
| `kintsugi` | divider | 1 | a gold repaired crack |
| `moss` | sidebar.ground, empty.art | 1 to 3 | soft moss cushions |
| `mist` | scroll.edge | 1 | a thickening mist |
| `laterite-speckle` | texture | 1 | rusty pitting |
| `stone-speckle` | texture | 1 | a rough stone floor |
| `salt-grain` | texture | 1 | weathered grain |
| `glyph-divider` | divider | 1 | a row of one glyph, default `"≈"` |
| `box-corners` | quote.frame | 1 | `╔ ╝` corners |
| `rhumb-lines` | editor.frame | 1 to 2 | compass roses with rhumb lines |
| `compass-rose` | editor.frame, empty.art | 1 to 2 | a compass rose |
| `soundings` | texture | 1 | depth numbers in the editor's margins |
| `seigaiha` | sidebar.ground, divider, empty.art | 1 to 3 | waves; on the sidebar, colours 2 and 3 are the ground |
| `sashiko` | quote.frame, divider | 1 | a running stitch |
| `indigo-dye` | titlebar.band, sidebar.ground | 2 | a dye gradient |
| `horizon` | titlebar.band, divider | 1 | a line fading at both ends |
| `tide-line` | divider | 1 to 2 | two wavering lines |
| `contours` | sidebar.ground, empty.art | 1 to 3 | sea contours with soundings; colours 2 and 3 are the ground |
| `harbour-glow` | editor.frame | 2 | the editor as lit paper in a sea (sea, glow) |
| `tide-sparkline` | status.ornament | 1 to 2 | your writing over the last two weeks (line, buoy) |
| `z-level` | status.ornament | 1 | the selected post's depth |
| `glyph` | marker.pinned, marker.new, empty.art | 0 to 1 | one glyph: `glyph(#e0a33a, "☼")` |
| `dot` | marker.pinned, marker.new | 0 to 1 | a small dot |
| `arch` | quote.frame | 1 | an arched room |
| `cushion` | row.selected, quote.frame | 1 | a moss-cushion shape |
| `outline` | row.selected, quote.frame | 1 | a hairline box |
| `double-rule` | quote.frame | 1 | a chart's double rule |
| `buoy` | quote.frame | 1 | squared, with a coloured top edge |
| `inset-bar` | row.selected, quote.frame | 1 | a bar down the left edge |
| `lantern` | row.selected | 1 to 2 | an outline with a warm glow |
| `rule` | quote.frame | 0 to 1 | a left rule on the quote colours |
| `line` | divider | 0 to 1 | a plain hairline |
| `svg` | titlebar.band, sidebar.ground, sidebar.top, editor.frame, divider, texture, empty.art | 0 to 1 | your own art (below) |

Every ornament takes `opacity=` (0 to 1).

### Your own SVG

Any picture slot can take your own art:

```
empty.art = svg(~/.config/blygger/themes/art/my-burrow.svg, #b0562a)
```

The path may start with `~/`, or be relative to the theme file. The SVG is
drawn in one colour (the one given, or a quiet default), like an icon: its
shapes are the mask. To keep a bad file from hanging the app, an ornament SVG
must be under 256 KB with at most 2,000 elements, and may not contain
entities, a DOCTYPE, scripts, embedded or linked images, `foreignObject`,
filters or more than 64 `<use>` elements. A file that breaks a rule is
reported in the banner, and the slot stays empty.

## Checking a theme

`blygger +validate-config` reports problems in the config and in every theme
file, with file and line: unknown keys (with a suggestion), bad colours,
ornaments in the wrong slot, a wrong number of colours, unknown fonts and SVGs
that break the rules. Unknown keys are warnings; bad values are errors that
leave that key as the layers below set it. Nothing in a theme file can stop
the app from starting.

The built-in themes are tested: each parses cleanly, copies out with
`+copy-theme` and reads back the same, and keeps body text at a contrast of at
least 4.5:1 against its background (and the sidebar's and selected row's text
against theirs, as well as the reading side's: selected rows on the page,
badges, notices, quote boxes and the reader's paper).
