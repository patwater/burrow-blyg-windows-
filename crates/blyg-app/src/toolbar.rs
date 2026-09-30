//! The optional toolbar (`show-buttons`, docs/SPEC.md § Buttons).
//!
//! Keyboard-first, not keyboard-only: a quiet row of icons and short labels
//! in the title bar, generated from `keymap::TOOLBAR` and the keymap table
//! (icon, label, key), so buttons, menus and shortcuts can't drift. A click
//! dispatches the row's action, exactly as the key does; every tooltip shows
//! the key; a disabled button says why, using the same view-model rules the
//! keys use (`vm::publish_decision`, `vm::make_draft_blocked`).
//!
//! A child module of `app` so it can read `MainView`. `app.rs` keeps only
//! small hooks marked `// --- buttons ---`. With `show-buttons = false`
//! nothing here draws, and the window is the minimal layout.
//!
//! Layout: the row sits between the traffic lights and the reading-view
//! switcher on the right. The centred window title shows only when the row
//! leaves it room; when even the labels don't fit, the row drops to icons.

use blyg_core::config::NewNote;
use blyg_core::{Item, Status};

use super::reading::View as Screen;
use super::studio::ViewMode;
use super::*;
use crate::keymap::{self, Keybind};

macro_rules! icon {
    ($name:literal) => {
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/",
            $name,
            ".svg"
        ))
    };
}

/// The bundled icons (Lucide, ISC; see packaging/THIRD_PARTY.md).
pub const ICONS: &[(&str, &[u8])] = &[
    ("plus", icon!("plus")),
    ("file-up", icon!("file-up")),
    ("send", icon!("send")),
    ("panel-left", icon!("panel-left")),
    ("columns-3", icon!("columns-3")),
    ("columns-2", icon!("columns-2")),
    ("rotate-ccw-clock", icon!("rotate-ccw-clock")),
    ("sparkles", icon!("sparkles")),
    ("zap", icon!("zap")),
    ("sticky-note", icon!("sticky-note")),
    ("trash-2", icon!("trash-2")),
    ("archive-x", icon!("archive-x")),
];

pub fn icon_svg(name: &str) -> Option<&'static [u8]> {
    ICONS.iter().find(|(n, _)| *n == name).map(|(_, b)| *b)
}

/// Where the row starts: clear of the traffic lights.
const LEFT: f32 = 78.;
const LABEL_SIZE: f32 = 11.5;
const ICON: f32 = 13.;
/// A button: padding either side, icon, gap, label.
const PAD_LABEL: f32 = 6.;
const PAD_ICON: f32 = 5.;
const ICON_GAP: f32 = 4.;
const BUTTON_GAP: f32 = 1.;
/// A hairline between groups, with its margins.
const SEPARATOR: f32 = 9.;

/// One button as drawn: what it does, and whether (and why not) it can.
#[derive(Debug, Clone)]
pub struct Button {
    pub action: &'static str,
    pub icon: &'static str,
    pub label: &'static str,
    /// "Publish  ⌘⏎".
    pub tooltip: String,
    /// Why it's disabled; `None` = enabled.
    pub reason: Option<String>,
    /// The current view (Write / Preview / Full editor), or the open panel.
    pub active: bool,
    pub group: usize,
    new_action: fn() -> Box<dyn Action>,
}

impl Button {
    pub fn enabled(&self) -> bool {
        self.reason.is_none()
    }
}

/// Everything the buttons' rules read, so they can be tested without a window.
pub struct Facts<'a> {
    pub current: Option<&'a Item>,
    pub view: ViewMode,
    pub screen: Screen,
    /// Reading screen: a post is open (⌘Y toggles its version list).
    pub reading_opened: bool,
    /// Your own post's version list is showing.
    pub versions_open: bool,
    /// Some AI provider is enabled (or a test provider is set).
    pub ai_enabled: bool,
    pub publishing: bool,
    /// A sheet (publish, settings, connect, …) is up.
    pub sheet_open: bool,
    pub new_note: NewNote,
}

pub const NEED_POSTS: &str = "Back to Posts first (⌘L)";
pub const NEED_POST: &str = "Select a post first";
pub const CLOSE_SHEET: &str = "Close the sheet first (esc)";
pub const TOO_LONG: &str = "Too long for a fragment. ⌘T makes it a thread";
pub const NEED_AI: &str = "Enable AI in Settings › AI accounts";
pub const NOT_SCRATCH: &str = "Make draft is for scratch notes";
pub const ALREADY_WITHDRAWN: &str = "Already withdrawn";

/// Whether `action`'s button shows at all. Delete and Withdraw share a slot
/// (docs/SPEC.md rule 5): a published post shows Withdraw, never Delete.
pub fn visible(action: &str, f: &Facts) -> bool {
    let published = f
        .current
        .is_some_and(|i| vm::discard(i) != vm::Discard::Delete);
    match action {
        "DeleteDraft" => !published,
        "Withdraw" => published,
        _ => true,
    }
}

/// Why `action`'s button is disabled (`None`: it's enabled), and whether it
/// shows as active.
pub fn rule(action: &str, f: &Facts) -> (Option<String>, bool) {
    let posts = f.screen == Screen::Posts;
    let gate = |r: Option<String>| -> Option<String> {
        if f.sheet_open {
            Some(CLOSE_SHEET.into())
        } else if !posts {
            Some(crate::keymap::hint(NEED_POSTS).into())
        } else {
            r
        }
    };
    let need_item = |then: &dyn Fn(&Item) -> Option<String>| match f.current {
        None => Some(NEED_POST.to_string()),
        Some(i) => then(i),
    };
    match action {
        "ShowCapture" => (None, false),
        "NewDraft" => (gate(None), false),
        "MakeDraft" => (
            gate(need_item(&|i| match vm::make_draft_blocked(i) {
                // Not a scratch note: say what the button is for, and what it is.
                Some(why) if i.status != Status::Scratch => Some(format!("{NOT_SCRATCH}. {why}")),
                other => other.map(str::to_string),
            })),
            false,
        ),
        "Publish" => (
            gate(need_item(&|i| {
                if f.publishing {
                    return Some("Publishing…".into());
                }
                match vm::publish_decision(i) {
                    PublishDecision::Shake => Some(crate::keymap::hint(TOO_LONG).into()),
                    PublishDecision::Empty => Some("Nothing to publish yet".into()),
                    PublishDecision::AlreadyPublished(v) => {
                        Some(format!("Already published as v{v}, with no edits since"))
                    }
                    PublishDecision::Sheet { .. } => None,
                }
            })),
            false,
        ),
        "ViewWrite" => (gate(None), posts && f.view == ViewMode::Write),
        "ViewSplit" => (gate(None), posts && f.view == ViewMode::Split),
        "ViewStudio" => (
            gate(None),
            posts && matches!(f.view, ViewMode::Studio | ViewMode::Focus),
        ),
        "ShowVersions" => {
            if f.screen == Screen::Reading && !f.sheet_open {
                let r = (!f.reading_opened).then(|| "Open a post first".to_string());
                return (r, false);
            }
            let r = gate(need_item(&|i| {
                (i.version == 0 || i.server_id.is_none())
                    .then(|| "Not published yet, so there's no history".to_string())
            }));
            // The list can close again while it's open.
            if f.versions_open && posts && !f.sheet_open {
                return (None, true);
            }
            (r, false)
        }
        // --- delete & withdraw ---
        "DeleteDraft" => (
            gate(need_item(&|i| {
                (vm::discard(i) != vm::Discard::Delete)
                    .then(|| "Published posts are withdrawn, not deleted".to_string())
            })),
            false,
        ),
        "Withdraw" => (
            gate(need_item(&|i| match vm::discard(i) {
                vm::Discard::Withdraw => None,
                vm::Discard::AlreadyWithdrawn => Some(ALREADY_WITHDRAWN.into()),
                vm::Discard::Delete => Some("Not published: a draft is deleted instead".into()),
            })),
            false,
        ),
        "AiGenerate" => (
            gate(if !f.ai_enabled {
                Some(NEED_AI.into())
            } else {
                need_item(&|_| None)
            }),
            false,
        ),
        _ => (None, false),
    }
}

/// The tooltip's name for a button (New follows `new-note`).
fn tip_label(k: &Keybind, new_note: NewNote) -> &'static str {
    match (k.action, new_note) {
        ("NewDraft", NewNote::Draft) => "New draft",
        ("NewDraft", NewNote::Scratch) => "New scratch note",
        ("MakeDraft", _) => "Make draft",
        ("ViewSplit", _) => "Preview: list + editor + preview",
        ("ViewStudio", _) => "Full editor: editor + preview",
        ("ViewWrite", _) => "Write: list + editor",
        ("AiGenerate", _) => "Generate (AI)",
        ("ShowCapture", _) => "Quick capture",
        ("DeleteDraft", _) => "Delete draft or scratch note…",
        ("Withdraw", _) => "Withdraw…",
        _ => k.button.unwrap_or(k.label),
    }
}

/// The toolbar's buttons, in order, from the keymap table.
pub fn buttons(f: &Facts, capture_hotkey: &str) -> Vec<Button> {
    let mut out = Vec::new();
    for (group, actions) in keymap::TOOLBAR.iter().enumerate() {
        for action in *actions {
            if !visible(action, f) {
                continue;
            }
            let Some(k) = keymap::button_row(action) else {
                continue;
            };
            let (Some(label), Some(icon), Some(new_action)) = (k.button, k.icon, k.new_action)
            else {
                continue;
            };
            let (reason, active) = rule(action, f);
            out.push(Button {
                action: k.action,
                icon,
                label,
                tooltip: keymap::tooltip(tip_label(&k, f.new_note), &k, capture_hotkey),
                reason,
                active,
                group,
                new_action,
            });
        }
    }
    out
}

/// How the row fits the title bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    /// Icons and labels; `title`: the centred window title fits too.
    Labels { title: bool },
    /// Icons only (narrow windows).
    Icons { title: bool },
}

impl Fit {
    pub fn labels(self) -> bool {
        matches!(self, Fit::Labels { .. })
    }
    pub fn title(self) -> bool {
        match self {
            Fit::Labels { title } | Fit::Icons { title } => title,
        }
    }
}

/// Widths in px: the window, the row with labels and without, the view
/// switcher on the right (with its margin), and the title.
pub fn fit(window: f32, labels: f32, icons: f32, switcher: f32, title: f32) -> Fit {
    let title_left = (window - title) / 2. - 16.;
    let room = window - switcher - 12.;
    let end = |w: f32| LEFT + w;
    if end(labels) <= title_left {
        Fit::Labels { title: true }
    } else if end(labels) <= room {
        Fit::Labels { title: false }
    } else {
        Fit::Icons {
            title: end(icons) <= title_left,
        }
    }
}

/// The row's width with labels (`label_widths` = each button's text) or icons only.
pub fn row_width(buttons: &[Button], label_widths: Option<&[f32]>) -> f32 {
    let mut w = 0.;
    for (i, b) in buttons.iter().enumerate() {
        if i > 0 {
            w += if buttons[i - 1].group != b.group {
                SEPARATOR
            } else {
                BUTTON_GAP
            };
        }
        w += match label_widths {
            Some(lw) => PAD_LABEL * 2. + ICON + ICON_GAP + lw[i],
            None => PAD_ICON * 2. + ICON,
        };
    }
    w
}

/// A tooltip: the name and key, and on a second line why it's disabled.
pub struct ButtonTip {
    pub text: String,
    pub reason: Option<String>,
}

impl Render for ButtonTip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(7.))
            .py(px(3.))
            .rounded(px(5.))
            .bg(gpui_kit::black().opacity(0.85))
            .text_color(gpui_kit::white())
            .font_family("Inter")
            .text_size(px(11.))
            .child(self.text.clone())
            .when_some(self.reason.clone(), |d, r| {
                d.child(div().text_color(gpui_kit::white().opacity(0.72)).child(r))
            })
    }
}

/// One quiet button: icon, and a label unless `icons_only`. Shared with the
/// quick-capture row.
#[allow(clippy::too_many_arguments)]
pub fn button_element(
    id: SharedString,
    icon: &'static str,
    label: Option<&'static str>,
    key_hint: Option<String>,
    tooltip: String,
    reason: Option<String>,
    active: bool,
    p: &Palette,
) -> Stateful<Div> {
    let enabled = reason.is_none();
    let (ink, muted, sel, hover) = (p.ink, p.muted, p.sel, p.line);
    let color = if !enabled {
        muted.opacity(0.45)
    } else if active {
        ink
    } else {
        muted
    };
    let pad = if label.is_some() { PAD_LABEL } else { PAD_ICON };
    let tip = (tooltip, reason);
    div()
        .id(id.clone())
        .debug_selector(move || id.to_string())
        .h(px(22.))
        .px(px(pad))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(ICON_GAP))
        .rounded(px(5.))
        .text_size(px(LABEL_SIZE))
        .text_color(color)
        .when(active, |d| d.bg(sel))
        .when(enabled, |d| {
            d.cursor_pointer()
                .hover(move |s| s.text_color(ink).bg(if active { sel } else { hover }))
        })
        .when_some(icon_svg(icon), |d, bytes| {
            d.child(
                svg()
                    .data(bytes)
                    .size(px(ICON))
                    .flex_none()
                    .text_color(color),
            )
        })
        .when_some(label, |d, l| d.child(l))
        .when_some(key_hint, |d, k| {
            d.child(div().text_color(muted.opacity(0.8)).child(k))
        })
        .tooltip(move |_, cx| {
            cx.new(|_| ButtonTip {
                text: tip.0.clone(),
                reason: tip.1.clone(),
            })
            .into()
        })
}

impl MainView {
    fn toolbar_facts(&self, cx: &App) -> Facts<'_> {
        let ai_enabled = cx
            .try_global::<crate::ai::AiGlobal>()
            .is_some_and(|g| g.test_provider.is_some())
            || cx
                .try_global::<crate::settings::AppConfig>()
                .is_some_and(|c| !c.store.config().ai_enabled().is_empty());
        Facts {
            current: self.current.as_ref(),
            view: self.studio.view,
            screen: self.reading.view,
            reading_opened: self.reading.opened.is_some(),
            versions_open: self.reading.own.is_some(),
            ai_enabled,
            publishing: self.publishing,
            sheet_open: self.sheet.is_some() || self.reading.sheet.is_some(),
            new_note: self.prefs.new_note,
        }
    }

    /// The toolbar's buttons as they'd be drawn now (tests read this too).
    pub(crate) fn toolbar_buttons(&self, cx: &App) -> Vec<Button> {
        buttons(&self.toolbar_facts(cx), &self.prefs.hotkey)
    }

    /// A click: the row's action, dispatched the way its key is.
    pub(crate) fn toolbar_click(
        &mut self,
        action: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(b) = self
            .toolbar_buttons(cx)
            .into_iter()
            .find(|b| b.action == action)
        else {
            return;
        };
        if !b.enabled() {
            return;
        }
        // Dispatch from inside the window's tree, where the handlers are.
        if !self.focus.contains_focused(window, cx) {
            window.focus(&self.focus, cx);
        }
        window.dispatch_action((b.new_action)(), cx);
    }

    fn text_width(
        &self,
        text: &str,
        size: f32,
        weight: FontWeight,
        family: &str,
        window: &Window,
    ) -> f32 {
        let mut f = font(SharedString::from(family.to_string()));
        f.weight = weight;
        let run = TextRun {
            len: text.len(),
            font: f,
            color: self.palette.ink,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = window
            .text_system()
            .shape_line(text.to_string().into(), px(size), &[run], None);
        f32::from(line.width)
    }

    /// Hook: how the row fits right now (`None`: no toolbar).
    pub(crate) fn toolbar_fit(&self, window: &Window, cx: &App) -> Option<(Fit, Vec<Button>)> {
        if !self.prefs.show_buttons {
            return None;
        }
        let ui: &str = self.prefs.ui().family;
        let buttons = self.toolbar_buttons(cx);
        let widths: Vec<f32> = buttons
            .iter()
            .map(|b| self.text_width(b.label, LABEL_SIZE, FontWeight::NORMAL, ui, window))
            .collect();
        let labels = row_width(&buttons, Some(&widths));
        let icons = row_width(&buttons, None);
        // The view switcher (reading/mod.rs): Inter 11.5, 9 px padding a
        // side, 2 px gaps, a 5 px dot + 4 px gap on some tabs, 12 px margin.
        let switcher = Screen::ALL
            .iter()
            .map(|v| {
                self.text_width(v.label(), 11.5, FontWeight::NORMAL, "Inter", window) + 18. + 9.
            })
            .sum::<f32>()
            + 2. * (Screen::ALL.len() as f32 - 1.)
            + 12.;
        let title = self.text_width(&self.title, 12., FontWeight::MEDIUM, ui, window);
        let w = f32::from(window.viewport_size().width);
        Some((fit(w, labels, icons, switcher, title), buttons))
    }

    /// Hook: the toolbar, over the left of the title bar.
    pub(crate) fn render_toolbar(
        &self,
        fitted: Option<(Fit, Vec<Button>)>,
        ui_font: &SharedString,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (fit, buttons) = fitted?;
        let p = self.palette;
        let mut row = div()
            .id("toolbar")
            .debug_selector(|| "toolbar".into())
            .absolute()
            .top_0()
            .left(px(LEFT))
            .h(px(TITLEBAR_H))
            .flex()
            .items_center()
            .gap(px(BUTTON_GAP))
            .font_family(ui_font.clone());
        let mut last_group = None;
        for b in buttons {
            if last_group.is_some_and(|g| g != b.group) {
                row = row.child(
                    div()
                        .mx(px((SEPARATOR - BUTTON_GAP * 2. - 1.) / 2.))
                        .w(px(1.))
                        .h(px(12.))
                        .bg(p.line),
                );
            }
            last_group = Some(b.group);
            let action = b.action;
            let el = button_element(
                format!("tb-{action}").into(),
                b.icon,
                fit.labels().then_some(b.label),
                None,
                b.tooltip.clone(),
                b.reason.clone(),
                b.active,
                &p,
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.toolbar_click(action, window, cx);
            }));
            row = row.child(el);
        }
        Some(row.into_any_element())
    }

    /// Hook: Settings › Buttons, a row in the Appearance area.
    pub(crate) fn render_buttons_setting(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        let chip = |id: &'static str, label: &'static str, on: bool| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .px(px(10.))
                .py(px(3.))
                .rounded_full()
                .border_1()
                .cursor_pointer()
                .border_color(if on { p.accent } else { p.line })
                .when(on, |d| d.text_color(p.accent))
                .hover(|s| s.border_color(p.accent))
                .child(label)
        };
        let on = self.prefs.show_buttons;
        div()
            .flex()
            .gap(px(12.))
            .py(px(7.))
            .child(
                div()
                    .w(px(92.))
                    .flex_none()
                    .pt(px(4.))
                    .text_size(px(10.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.muted)
                    .child("BUTTONS"),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        chip("show-buttons-on", "Show toolbar buttons", on).on_click(cx.listener(
                            |this, _, window, cx| this.set_show_buttons(true, window, cx),
                        )),
                    )
                    .child(chip("show-buttons-off", "Keyboard only", !on).on_click(
                        cx.listener(|this, _, window, cx| this.set_show_buttons(false, window, cx)),
                    )),
            )
            .into_any_element()
    }

    /// Settings' toggle: `show-buttons`, written to the config file.
    pub(crate) fn set_show_buttons(
        &mut self,
        on: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.prefs.show_buttons == on {
            return;
        }
        self.prefs.show_buttons = on;
        self.apply_prefs(window, cx);
    }

    /// `BLYGGER_DEMO=tb-…` (fake mode, screenshots): `tb-settings`, `tb-main` (a draft
    /// open), `tb-long` (an over-limit fragment, Publish's tooltip up),
    /// `tb-capture` (the quick-capture panel's row), `tb-delete` / `tb-withdraw`
    /// (the confirmation sheets). `BLYGGER_SNAPSHOT_WIDTH`
    /// resizes the window first; `BLYGGER_DEMO_HOVER=<action>` points at a
    /// button (an in-app event, not an OS one) so its tooltip shows.
    pub(crate) fn toolbar_demo(
        &mut self,
        scenario: &str,
        n: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if n == 0
            && let Some(w) = std::env::var("BLYGGER_SNAPSHOT_WIDTH")
                .ok()
                .and_then(|w| w.parse::<f32>().ok())
        {
            let h = window.viewport_size().height;
            window.resize(size(px(w), h));
        }
        match (scenario, n) {
            ("tb-long", 0) => {
                self.open(&LocalId("01J9QK3".into()), window, cx);
                let extra = " The log keeps the weather so the keeper doesn't have to.".repeat(18);
                self.editor.update(cx, |s, cx| s.insert(extra, window, cx));
                self.after_edit(cx);
            }
            ("tb-settings", 0) => self.open_settings(&OpenSettings, window, cx),
            // --- delete & withdraw --- the confirmation sheets (discard.rs)
            ("tb-delete" | "tb-withdraw", 0) => self.discard_demo(scenario, window, cx),
            ("tb-capture", 0) => crate::capture::toggle(cx),
            ("tb-capture", 1) => crate::capture::demo_fill(
                "An idea on the stairs: tide tables as a writing prompt.",
                cx,
            ),
            ("tb-capture", 2) => {
                if let Ok(path) = std::env::var("BLYGGER_SNAPSHOT") {
                    crate::capture::demo_snapshot(&path, cx);
                }
                return;
            }
            _ => {}
        }
        if n == 2 {
            if let Ok(action) = std::env::var("BLYGGER_DEMO_HOVER") {
                self.demo_hover(&action, window, cx);
            }
            super::reading::demo::snapshot_later(window, cx);
        }
    }

    /// Move the pointer over a toolbar button, inside the app.
    fn demo_hover(&mut self, action: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((fit, buttons)) = self.toolbar_fit(window, cx) else {
            return;
        };
        let ui: &str = self.prefs.ui().family;
        let mut x = LEFT;
        for (i, b) in buttons.iter().enumerate() {
            if i > 0 {
                x += if buttons[i - 1].group != b.group {
                    SEPARATOR
                } else {
                    BUTTON_GAP
                };
            }
            let w = if fit.labels() {
                PAD_LABEL * 2.
                    + ICON
                    + ICON_GAP
                    + self.text_width(b.label, LABEL_SIZE, FontWeight::NORMAL, ui, window)
            } else {
                PAD_ICON * 2. + ICON
            };
            if b.action == action {
                let position = point(px(x + w / 2.), px(TITLEBAR_H / 2.));
                // Draw first: an offscreen window may still show an old frame,
                // whose buttons (and tooltips) the pointer would hit.
                cx.spawn_in(window, async move |_, cx| {
                    let _ = cx.update(|window, cx| {
                        window.draw(cx).clear(cx);
                        window.dispatch_event(
                            PlatformInput::MouseMove(MouseMoveEvent {
                                position,
                                pressed_button: None,
                                modifiers: Modifiers::default(),
                            }),
                            cx,
                        );
                    });
                })
                .detach();
                return;
            }
            x += w;
        }
    }
}

#[cfg(test)]
#[path = "toolbar_tests.rs"]
mod tests;
