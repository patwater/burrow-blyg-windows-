//! The composer's GPUI part, embedded by a host view over its textarea:
//! the @-mention popup, the `[[` / `![[` picker typed in the editor (the
//! host opens it, see `picker`), the spellcheck underlines, and the
//! spelling menu.
//!
//! The host renders the `Assist` entity as an overlay covering the
//! textarea, calls [`Assist::reset`] after replacing the text itself
//! (`set_value` sends no event), [`Assist::note_paste`] from its paste
//! handler, and routes ↑/↓/⏎/⇥/esc through [`Assist::handle_key`] first
//! while [`Assist::wants_keys`].
//!
//! Typing stays instant: an edit only slides the underlines along
//! (`spell::shift`) and restarts a timer. The check runs ~300 ms after
//! typing stops, on the lines that changed, on a background thread.

use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use blyg_core::Backend;
use gpui_kit::base::input::{InputEvent, RopeExt as _, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::mention::{self, Candidate};
use super::picker::{self, Filters, PickKind};
use super::spell::{self, SpellEngine};
use super::{SpellService, spellcheck_enabled};
use crate::theme::Palette;
use crate::vm;

/// How long typing must pause before a check.
pub const SPELL_DEBOUNCE: Duration = Duration::from_millis(300);
/// Suggestions the spelling menu shows.
const MAX_GUESSES: usize = 5;

/// The keys the popups take while open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssistKey {
    Up,
    Down,
    Enter,
    Tab,
    Escape,
}

struct MentionPopup {
    /// Byte offset of the typed `@`.
    at: usize,
    /// Every known blyg (`None` while they're being gathered).
    all: Option<Vec<Candidate>>,
    /// What matches the query now.
    shown: Vec<Candidate>,
    query: String,
    sel: usize,
}

/// The `[[` / `![[` picker, its query typed in the editor.
struct PickerPopup {
    kind: PickKind,
    /// Byte offset of the typed `[[` / `![[`.
    at: usize,
    /// The post being written (it can't name itself).
    exclude: Option<String>,
    query: String,
    rows: Vec<blyg_core::Pickable>,
    /// A search has answered (until then "no matches" isn't true yet).
    searched: bool,
    sel: usize,
    /// Blyg subscriptions for the "from" chip: (id, title).
    subs: Vec<(String, String)>,
    scroll: ScrollHandle,
}

enum MenuItem {
    Replace(String),
    Learn,
    Ignore,
}

struct SpellMenu {
    range: Range<usize>,
    word: String,
    items: Vec<MenuItem>,
    sel: usize,
    position: Point<Pixels>,
}

pub struct Assist {
    editor: Entity<TextareaState>,
    backend: Arc<dyn Backend>,
    palette: Palette,
    /// --- themes --- the interface font the popups use.
    ui_font: &'static str,
    /// The text as of the last event, to see what an edit changed.
    last: String,
    /// The next edit is a paste: it never opens the mention popup.
    pasting: bool,
    mention: Option<MentionPopup>,
    picker: Option<PickerPopup>,
    /// Bumped per picker search; a stale answer is dropped.
    pick_gen: usize,
    menu: Option<SpellMenu>,
    /// Misspelled byte ranges in `last`.
    misspelled: Vec<Range<usize>>,
    debounce: Option<Task<()>>,
    checking: Option<Task<()>>,
    /// Text changed while a check was running: plan again when it lands.
    recheck: bool,
    _subs: Vec<Subscription>,
}

impl Assist {
    pub fn new(
        editor: Entity<TextareaState>,
        backend: Arc<dyn Backend>,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subs = vec![cx.subscribe_in(&editor, window, Self::on_editor_event)];
        if cx.has_global::<SpellService>() {
            subs.push(cx.observe_global::<SpellService>(|this, cx| {
                if spellcheck_enabled(cx) {
                    this.schedule_check(Duration::ZERO, cx);
                } else {
                    this.misspelled.clear();
                    this.menu = None;
                    this.debounce = None;
                    this.checking = None;
                }
                cx.notify();
            }));
        }
        // Right-click (or ctrl-click): the editor has already put the caret
        // at the click; offer the spelling menu if it's on a flagged word.
        // The editor calls this from inside its own update, so defer once
        // more before reading it (reading it there panicked the app).
        let weak = cx.entity().downgrade();
        editor.update(cx, |s, _| {
            s.on_context_menu(std::rc::Rc::new(move |_, _, position, window, cx| {
                let weak = weak.clone();
                window.defer(cx, move |window, cx| {
                    let _ = weak.update(cx, |a, cx| a.open_spell_menu(position, window, cx));
                });
            }));
        });
        let last = editor.read(cx).value().to_string();
        let mut this = Self {
            editor,
            backend,
            palette,
            ui_font: crate::prefs::UI_FONTS[0].family,
            last,
            pasting: false,
            mention: None,
            picker: None,
            pick_gen: 0,
            menu: None,
            misspelled: Vec::new(),
            debounce: None,
            checking: None,
            recheck: false,
            _subs: subs,
        };
        this.schedule_check(Duration::ZERO, cx);
        this
    }

    pub fn set_palette(&mut self, p: Palette) {
        self.palette = p;
    }

    /// The interface font (the user's, or the theme's) for the popups.
    pub fn set_ui_font(&mut self, family: &'static str) {
        self.ui_font = family;
    }

    /// The host replaced the text without an event (`set_value`).
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.last = self.editor.read(cx).value().to_string();
        self.mention = None;
        self.picker = None;
        self.menu = None;
        self.misspelled.clear();
        self.schedule_check(Duration::ZERO, cx);
        cx.notify();
    }

    /// A paste is about to land: it must not open the mention popup.
    pub fn note_paste(&mut self) {
        self.pasting = true;
    }

    fn on_editor_event(
        &mut self,
        _: &Entity<TextareaState>,
        ev: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            InputEvent::Change => self.edited(window, cx),
            InputEvent::Blur
                if self.mention.is_some() || self.menu.is_some() || self.picker.is_some() =>
            {
                self.mention = None;
                self.picker = None;
                self.menu = None;
                cx.notify();
            }
            _ => {}
        }
    }

    /// The text changed (typing, a paste, undo, a splice).
    fn edited(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let (text, cursor) = {
            let s = self.editor.read(cx);
            (s.value().to_string(), s.cursor())
        };
        let pasting = std::mem::take(&mut self.pasting);
        let (edit, inserted) = vm::splice(&self.last, &text);
        spell::shift(&mut self.misspelled, &edit, inserted.len());
        self.menu = None;
        let old = std::mem::replace(&mut self.last, text);
        let text = &self.last;

        let mut search = false;
        if let Some(pk) = self.picker.as_mut() {
            match picker::query(text, pk.at, pk.kind, cursor) {
                Some(q) if q != pk.query => {
                    pk.query = q.to_string();
                    pk.sel = 0;
                    search = true;
                }
                Some(_) => {}
                None => self.picker = None,
            }
        }
        if let Some(m) = self.mention.as_mut() {
            match mention::query(text, m.at, cursor) {
                Some(q) => {
                    m.query = q.to_string();
                    m.sel = 0;
                    if let Some(all) = &m.all {
                        m.shown = mention::filter(all, q);
                        if m.shown.is_empty() && q.contains(char::is_whitespace) {
                            self.mention = None;
                        }
                    }
                }
                None => self.mention = None,
            }
        } else if !pasting && let Some(at) = mention::trigger(&old, text, cursor) {
            self.open_mentions(at, cx);
        }
        if search {
            self.search_picker(cx);
        }
        self.schedule_check(SPELL_DEBOUNCE, cx);
        cx.notify();
    }

    // ------------------------------------------------------------ keys

    /// Whether a popup is up and wants ↑/↓/⏎/⇥/esc.
    pub fn wants_keys(&self) -> bool {
        self.mention.is_some() || self.menu.is_some() || self.picker.is_some()
    }

    /// Handle a key while a popup is up. `false` lets it through to the editor.
    pub fn handle_key(
        &mut self,
        key: AssistKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(menu) = self.menu.as_mut() {
            let n = menu.items.len();
            match key {
                AssistKey::Up => menu.sel = menu.sel.saturating_sub(1),
                AssistKey::Down => menu.sel = (menu.sel + 1).min(n.saturating_sub(1)),
                AssistKey::Enter | AssistKey::Tab => {
                    let sel = menu.sel;
                    self.pick_menu(sel, window, cx);
                }
                AssistKey::Escape => self.menu = None,
            }
            cx.notify();
            return true;
        }
        if let Some(pk) = self.picker.as_mut() {
            let n = pk.rows.len();
            match key {
                // The typed brackets and query stay as plain text.
                AssistKey::Escape => self.picker = None,
                _ if n == 0 => {
                    if key != AssistKey::Tab {
                        self.picker = None;
                        cx.notify();
                        return false;
                    }
                }
                AssistKey::Up => pk.sel = pk.sel.saturating_sub(1),
                AssistKey::Down => pk.sel = (pk.sel + 1).min(n - 1),
                AssistKey::Enter | AssistKey::Tab => {
                    let sel = pk.sel;
                    self.pick_row(sel, window, cx);
                }
            }
            if let Some(pk) = self.picker.as_ref() {
                pk.scroll.scroll_to_item(pk.sel);
            }
            cx.notify();
            return true;
        }
        let Some(m) = self.mention.as_mut() else {
            return false;
        };
        let n = m.shown.len();
        match key {
            AssistKey::Escape => {
                // The typed `@query` stays as plain text.
                self.mention = None;
            }
            _ if n == 0 => {
                // Nothing to choose: the key does what it always does.
                if key != AssistKey::Tab {
                    self.mention = None;
                    cx.notify();
                    return false;
                }
            }
            AssistKey::Up => m.sel = m.sel.saturating_sub(1),
            AssistKey::Down => m.sel = (m.sel + 1).min(n - 1),
            AssistKey::Enter | AssistKey::Tab => {
                let sel = m.sel;
                self.pick_mention(sel, window, cx);
            }
        }
        cx.notify();
        true
    }

    // ------------------------------------------------------------ picker

    /// Open the picker for brackets typed at `at` (the host decided that
    /// the query is typed in the editor, see `picker_typing`).
    pub fn open_picker(
        &mut self,
        kind: PickKind,
        at: usize,
        exclude: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.mention = None;
        self.menu = None;
        let subs = picker::blyg_subs(&self.backend.subscriptions());
        self.picker = Some(PickerPopup {
            kind,
            at,
            exclude,
            query: String::new(),
            rows: Vec::new(),
            searched: false,
            sel: 0,
            subs,
            scroll: ScrollHandle::new(),
        });
        self.search_picker(cx);
        cx.notify();
    }

    /// Search again for the query and the current filters, off the typing
    /// path (it's a local index lookup, but typing must never wait).
    fn search_picker(&mut self, cx: &mut Context<Self>) {
        let Some(pk) = self.picker.as_ref() else {
            return;
        };
        self.pick_gen += 1;
        let gen_ = self.pick_gen;
        let q = Filters::get(cx).query(&pk.query, pk.exclude.clone());
        let backend = self.backend.clone();
        let found = cx.background_spawn(async move { backend.pick_search(&q) });
        cx.spawn(async move |this, cx| {
            let rows = found.await;
            let _ = this.update(cx, |a, cx| {
                if a.pick_gen != gen_ {
                    return;
                }
                if let Some(pk) = a.picker.as_mut() {
                    pk.rows = rows;
                    pk.searched = true;
                    pk.sel = pk.sel.min(pk.rows.len().saturating_sub(1));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn pick_row(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pk) = self.picker.take() else { return };
        let Some(row) = pk.rows.get(i) else { return };
        let (text, cursor) = {
            let s = self.editor.read(cx);
            (s.value().to_string(), s.cursor())
        };
        if picker::query(&text, pk.at, pk.kind, cursor).is_none() {
            return;
        }
        let (new, caret) = picker::insert(&text, pk.at, cursor, pk.kind, &row.id);
        self.splice(&text, &new, caret, window, cx);
    }

    // ------------------------------------------------------------ mentions

    fn open_mentions(&mut self, at: usize, cx: &mut Context<Self>) {
        self.mention = Some(MentionPopup {
            at,
            all: None,
            shown: Vec::new(),
            query: String::new(),
            sel: 0,
        });
        // Reading the store can take a few ms: off the typing path.
        let backend = self.backend.clone();
        let gathered = cx.background_spawn(async move {
            let subs = backend.subscriptions();
            let reading = backend.reading();
            let profiles = backend.cached_profiles();
            let items = backend.items();
            let own = backend.base_url();
            let src = mention::Sources {
                subscriptions: &subs,
                reading: &reading,
                profiles: &profiles,
                items: &items,
                own_origin: own.as_deref(),
            };
            mention::gather(&src, chrono::Utc::now())
        });
        cx.spawn(async move |this, cx| {
            let all = gathered.await;
            let _ = this.update(cx, |a, cx| {
                if let Some(m) = a.mention.as_mut().filter(|m| m.at == at) {
                    m.shown = mention::filter(&all, &m.query);
                    m.all = Some(all);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The mention popup is up (the tutorial watches for it).
    pub fn mention_open(&self) -> bool {
        self.mention.is_some()
    }

    /// The spelling menu is up (the tutorial watches for it).
    pub fn menu_open(&self) -> bool {
        self.menu.is_some()
    }

    /// What the mention popup shows now (tests read it).
    #[cfg(test)]
    pub fn mention_labels(&self) -> Option<Vec<String>> {
        self.mention
            .as_ref()
            .map(|m| m.shown.iter().map(|c| c.label().to_string()).collect())
    }

    fn pick_mention(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.mention.take() else { return };
        let Some(c) = m.shown.get(i) else { return };
        let (text, cursor) = {
            let s = self.editor.read(cx);
            (s.value().to_string(), s.cursor())
        };
        if mention::query(&text, m.at, cursor).is_none() {
            return;
        }
        let (new, caret) = mention::insert(&text, m.at, cursor, c);
        self.splice(&text, &new, caret, window, cx);
    }

    /// Apply `old -> new` as one undoable replacement, caret at `caret`.
    fn splice(
        &mut self,
        old: &str,
        new: &str,
        caret: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (range, insert) = vm::splice(old, new);
        let insert = insert.to_string();
        self.editor.update(cx, |s, cx| {
            s.set_selected_range(range, cx);
            s.replace(insert, window, cx);
            let c = caret.min(s.text().len());
            s.set_selected_range(c..c, cx);
        });
    }

    // ------------------------------------------------------------ spelling

    /// The underlined ranges (tests read them).
    #[cfg(test)]
    pub fn misspelled(&self) -> &[Range<usize>] {
        &self.misspelled
    }

    fn engine(cx: &App) -> Option<Arc<dyn SpellEngine>> {
        let s = cx.try_global::<SpellService>()?;
        s.enabled.then(|| s.engine.clone()).flatten()
    }

    fn schedule_check(&mut self, delay: Duration, cx: &mut Context<Self>) {
        if Self::engine(cx).is_none() {
            self.misspelled.clear();
            return;
        }
        if delay.is_zero() {
            self.debounce = None;
            self.run_check(cx);
            return;
        }
        self.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |a, cx| a.run_check(cx));
        }));
    }

    /// Underline what the cache knows; send the lines it doesn't to the
    /// checker in the background, then come back.
    fn run_check(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = Self::engine(cx) else {
            self.misspelled.clear();
            return;
        };
        if self.checking.is_some() {
            self.recheck = true;
            return;
        }
        let (text, cursor) = {
            let s = self.editor.read(cx);
            (s.value().to_string(), s.cursor())
        };
        let masked = spell::mask(&text);
        let plan = cx.global::<SpellService>().cache.plan(&masked);
        if plan.missing.is_empty() {
            self.last = text;
            self.misspelled = spell::settle(plan.known, cursor);
            cx.notify();
            return;
        }
        let missing = plan.missing;
        let checked = cx.background_spawn(async move {
            missing
                .into_iter()
                .map(|line| {
                    let r = engine.check(&line);
                    (line, r)
                })
                .collect::<Vec<_>>()
        });
        self.checking = Some(cx.spawn(async move |this, cx| {
            let results = checked.await;
            let _ = this.update(cx, |a, cx| {
                let cache = &mut cx.global_mut::<SpellService>().cache;
                for (line, r) in results {
                    cache.insert(line, r);
                }
                a.checking = None;
                a.recheck = false;
                a.run_check(cx);
            });
        }));
    }

    fn open_spell_menu(
        &mut self,
        position: Point<Pixels>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mention = None;
        let Some(engine) = Self::engine(cx) else {
            return;
        };
        let (text, cursor, sel) = {
            let s = self.editor.read(cx);
            (s.value().to_string(), s.cursor(), s.selected_range())
        };
        if text != self.last {
            return;
        }
        let offset = if sel.is_empty() { cursor } else { sel.start };
        let Some(range) = spell::at(&self.misspelled, offset) else {
            return;
        };
        let Some(word) = text.get(range.clone()).map(str::to_string) else {
            return;
        };
        let mut items: Vec<MenuItem> = engine
            .guesses(&word)
            .into_iter()
            .take(MAX_GUESSES)
            .map(MenuItem::Replace)
            .collect();
        items.push(MenuItem::Learn);
        items.push(MenuItem::Ignore);
        self.menu = Some(SpellMenu {
            range,
            word,
            items,
            sel: 0,
            position,
        });
        cx.notify();
    }

    /// The spelling menu's items as shown (tests read them).
    #[cfg(test)]
    pub fn menu_labels(&self) -> Option<Vec<String>> {
        self.menu
            .as_ref()
            .map(|m| m.items.iter().map(|i| item_label(i).to_string()).collect())
    }

    /// Open the spelling menu for the flagged word at `offset` (tests and
    /// the demo; a right-click goes through the editor).
    pub fn open_spell_menu_at(
        &mut self,
        offset: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let position = self
            .editor
            .read(cx)
            .range_to_bounds(&(offset..offset))
            .map_or(point(px(200.), px(200.)), |b| b.bottom_left());
        self.editor
            .update(cx, |s, cx| s.set_selected_range(offset..offset, cx));
        self.open_spell_menu(position, window, cx);
    }

    pub fn pick_menu(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.menu.take() else { return };
        let Some(item) = menu.items.get(i) else {
            return;
        };
        let Some(engine) = cx
            .try_global::<SpellService>()
            .and_then(|s| s.engine.clone())
        else {
            return;
        };
        match item {
            MenuItem::Replace(w) => {
                let text = self.editor.read(cx).value().to_string();
                if text.get(menu.range.clone()) != Some(menu.word.as_str()) {
                    return;
                }
                let mut new = text.clone();
                new.replace_range(menu.range.clone(), w);
                let caret = menu.range.start + w.len();
                self.splice(&text, &new, caret, window, cx);
            }
            MenuItem::Learn | MenuItem::Ignore => {
                if matches!(item, MenuItem::Learn) {
                    engine.learn(&menu.word);
                } else {
                    engine.ignore(&menu.word);
                }
                cx.global_mut::<SpellService>()
                    .cache
                    .forget_word(&menu.word);
                let text = &self.last;
                self.misspelled
                    .retain(|r| text.get(r.clone()) != Some(menu.word.as_str()));
                self.schedule_check(Duration::ZERO, cx);
            }
        }
        cx.notify();
    }

    // ------------------------------------------------------------ render

    fn paint_underlines(
        editor: &Entity<TextareaState>,
        ranges: &[Range<usize>],
        text_len: usize,
        color: Hsla,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let s = editor.read(cx);
        if s.text().len() != text_len || ranges.is_empty() {
            return; // stale until the next event
        }
        let (Some(rows), Some(lh)) = (s.visible_row_range(), s.line_height()) else {
            return;
        };
        let rope = s.text();
        let lines = rope.lines_len();
        let first = rope.line_start_offset(rows.start.min(lines.saturating_sub(1)));
        let last = if rows.end >= lines {
            rope.len()
        } else {
            rope.line_start_offset(rows.end)
        };
        let font = window.text_style().font_size.to_pixels(window.rem_size());
        let style = UnderlineStyle {
            thickness: px(1.),
            color: Some(color),
            wavy: true,
        };
        let rows_of = |r: Range<usize>| -> Vec<Bounds<Pixels>> {
            let Some(b) = s.range_to_bounds(&r) else {
                return vec![];
            };
            if b.size.height <= lh * 1.5 {
                return vec![b];
            }
            // Soft-wrapped mid-word: underline each row's part.
            let text = rope.slice(r.clone()).to_string();
            let mut out = Vec::new();
            let mut seg = r.start;
            let mut prev = r.start;
            for (i, ch) in text.char_indices().skip(1).chain([(text.len(), ' ')]) {
                let k = r.start + i;
                let fits = s
                    .range_to_bounds(&(seg..k))
                    .is_some_and(|b| b.size.height <= lh * 1.5);
                if !fits {
                    out.extend(s.range_to_bounds(&(seg..prev)));
                    seg = prev;
                }
                prev = k;
                let _ = ch;
            }
            out.extend(s.range_to_bounds(&(seg..r.end)));
            out
        };
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for r in ranges.iter().filter(|r| r.start >= first && r.end <= last) {
                for b in rows_of(r.clone()) {
                    let y = b.top() + (lh + font) / 2. + px(1.);
                    window.paint_underline(point(b.left(), y), b.size.width, &style);
                }
            }
        });
    }

    fn popup_card(&self) -> Div {
        let p = self.palette;
        // --- themes --- raised on dark themes, with an edge and a shadow
        // that show there; the chosen row is `pick()`, never a dark `sel`.
        div()
            .bg(p.raised())
            .border_1()
            .border_color(p.edge())
            .rounded(px(8.))
            // A shadow that falls below, never over the line above (where
            // the flagged word's underline is).
            .shadow(vec![BoxShadow {
                color: p.drop().opacity(0.6),
                offset: point(px(0.), px(8.)),
                blur_radius: px(16.),
                spread_radius: px(-8.),
                inset: false,
            }])
            .p(px(4.))
            .text_size(px(13.))
            .line_height(relative(1.35))
            .font_family(SharedString::from(self.ui_font))
            .text_color(p.ink)
    }

    fn anchor_below(&self, offset: usize, cx: &App) -> Option<Point<Pixels>> {
        let s = self.editor.read(cx);
        let b = s.range_to_bounds(&(offset..offset))?;
        Some(point(b.left() - px(8.), b.bottom() + px(2.)))
    }

    fn render_mentions(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let m = self.mention.as_ref()?;
        let pos = self.anchor_below(m.at, cx)?;
        let p = self.palette;
        let rows = m.shown.iter().enumerate().map(|(i, c)| {
            div()
                .id(("mention", i))
                .px(px(8.))
                .py(px(4.))
                .rounded(px(5.))
                .cursor_pointer()
                .when(i == m.sel, |d| d.bg(p.pick()))
                .when(i != m.sel, |d| d.hover(|s| s.bg(p.hover())))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.pick_mention(i, window, cx);
                    cx.notify();
                }))
                .child(div().truncate().child(c.label().to_string()))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(p.muted)
                        .truncate()
                        .child(format!("{} · {}", c.host, c.why)),
                )
        });
        let empty = m.all.is_some() && m.shown.is_empty();
        let card = self
            .popup_card()
            .id("mention-popup")
            .w(px(300.))
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                // Keep the editor focused (and the popup open) on a click.
                window.prevent_default();
                cx.stop_propagation();
            })
            .children(rows)
            .when(empty, |d| {
                d.child(
                    div()
                        .px(px(8.))
                        .py(px(4.))
                        .italic()
                        .text_color(p.muted)
                        .child(format!("No blyg you know matches “{}”", m.query)),
                )
            })
            .when(!m.shown.is_empty(), |d| {
                d.child(
                    div()
                        .px(px(8.))
                        .pt(px(4.))
                        .pb(px(2.))
                        .text_size(px(11.))
                        .text_color(p.muted)
                        .child("⏎ links the blyg · esc keeps your text"),
                )
            });
        Some(
            deferred(
                anchored()
                    .position(pos)
                    .snap_to_window_with_margin(px(8.))
                    .child(card),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    fn render_picker(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pk = self.picker.as_ref()?;
        let pos = self.anchor_below(pk.at, cx)?;
        let p = self.palette;
        let weak = cx.entity().downgrade();
        let set: picker::OnFilters = std::rc::Rc::new(move |f: Filters, _, cx: &mut App| {
            f.set(cx);
            let _ = weak.update(cx, |a, cx| {
                if let Some(pk) = a.picker.as_mut() {
                    pk.sel = 0;
                }
                a.search_picker(cx);
                cx.notify();
            });
        });
        let weak = cx.entity().downgrade();
        let pick: picker::OnPick = std::rc::Rc::new(move |i, window, cx: &mut App| {
            let _ = weak.update(cx, |a, cx| {
                a.pick_row(i, window, cx);
                cx.notify();
            });
        });
        let filters = Filters::get(cx);
        let empty = pk.searched && pk.rows.is_empty();
        let card = self
            .popup_card()
            .id("picker-popup")
            .w(px(440.))
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                // Keep the editor focused (and the picker open) on a click.
                window.prevent_default();
                cx.stop_propagation();
            })
            .child(
                div()
                    .px(px(8.))
                    .pt(px(3.))
                    .pb(px(5.))
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .child(
                        div()
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(p.muted)
                            .child(pk.kind.heading()),
                    )
                    .child(picker::filter_row(p, &filters, &pk.subs, set)),
            )
            .child(
                div()
                    .id("picker-rows")
                    .max_h(px(300.))
                    .overflow_y_scroll()
                    .track_scroll(&pk.scroll)
                    .children(picker::rows(p, &pk.rows, pk.sel, pick)),
            )
            .when(empty, |d| {
                d.child(
                    div()
                        .px(px(8.))
                        .py(px(4.))
                        .italic()
                        .text_color(p.muted)
                        .child(if pk.query.trim().is_empty() {
                            "Nothing to offer here. Links and quotes come from your published posts and your blyg subscriptions.".to_string()
                        } else {
                            format!("Nothing held matches “{}”", pk.query.trim())
                        }),
                )
            })
            .child(
                div()
                    .px(px(8.))
                    .pt(px(4.))
                    .pb(px(2.))
                    .text_size(px(11.))
                    .text_color(p.muted)
                    .child(format!(
                        "type to search · ↑↓ choose · ⏎ {} · esc keeps your text",
                        pk.kind.enter_hint()
                    )),
            );
        Some(
            deferred(
                anchored()
                    .position(pos)
                    .snap_to_window_with_margin(px(8.))
                    .child(card),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    fn render_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let p = self.palette;
        let guesses = menu
            .items
            .iter()
            .filter(|i| matches!(i, MenuItem::Replace(_)))
            .count();
        let mut card = self
            .popup_card()
            .id("spell-menu")
            .min_w(px(180.))
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.menu = None;
                cx.notify();
            }));
        if guesses == 0 {
            card = card.child(
                div()
                    .px(px(8.))
                    .py(px(4.))
                    .italic()
                    .text_color(p.muted)
                    .child("No guesses"),
            );
        }
        for (i, item) in menu.items.iter().enumerate() {
            if i == guesses {
                card = card.child(div().my(px(3.)).h(px(1.)).bg(p.edge()));
            }
            card = card.child(
                div()
                    .id(("spell", i))
                    .px(px(8.))
                    .py(px(3.))
                    .rounded(px(5.))
                    .cursor_pointer()
                    .when(i == menu.sel, |d| d.bg(p.pick()))
                    .when(i != menu.sel, |d| d.hover(|s| s.bg(p.hover())))
                    .when(matches!(item, MenuItem::Replace(_)), |d| {
                        d.font_weight(FontWeight::SEMIBOLD)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.pick_menu(i, window, cx);
                    }))
                    .child(item_label(item).to_string()),
            );
        }
        Some(
            deferred(
                anchored()
                    .position(menu.position)
                    .snap_to_window_with_margin(px(8.))
                    .child(card),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}

fn item_label(i: &MenuItem) -> &str {
    match i {
        MenuItem::Replace(w) => w,
        MenuItem::Learn => "Learn Spelling",
        MenuItem::Ignore => "Ignore",
    }
}

impl Render for Assist {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.editor.read(cx).focus_handle(cx).is_focused(window);
        let editor = self.editor.clone();
        let ranges = self.misspelled.clone();
        let text_len = self.last.len();
        let color = self.palette.over;
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        Self::paint_underlines(
                            &editor, &ranges, text_len, color, bounds, window, cx,
                        )
                    },
                )
                .size_full(),
            )
            .when(focused, |d| {
                d.children(self.render_mentions(cx))
                    .children(self.render_picker(cx))
                    .children(self.render_menu(cx))
            })
    }
}
