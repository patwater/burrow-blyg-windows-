//! The interactive tutorial: an overlay on the real window that runs the
//! script in `steps.rs` on a fresh FakeBackend.
//!
//! Starting it *parks* the real backend: the app's `SwitchBackend` is
//! switched to the tutorial's FakeBackend (so quick capture and the status
//! bar follow), and the real backend's events are held back. Finishing
//! puts the real backend back, replays any held conflict or error, and
//! restores the query, selection and view mode. ⌘G / ⇧⌘G get a canned
//! provider meanwhile, so no AI is ever called.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use blyg_ai::{GenRequest, GenResult, Provider, ProviderKind};
use blyg_core::{Backend, CoreEvent, Kind, LocalId, Status};
use gpui_kit::base::input::{Enter, MoveDown, MoveUp};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::steps::{self, Key, Region, STEPS, Setup};
use crate::app::scratch::MakeDraft;
use crate::app::studio::{ViewMode, ViewSplit, ViewStudio, ViewWrite};
use crate::app::{
    MainView, Mode as EditMode, OMNI_H, Publish, STATUS_H, Sheet, TITLEBAR_H, ToggleKind,
};
use crate::connection::{Connection, Mode, SwitchBackend};
use crate::fake::FakeBackend;

/// What the tutorial set aside, to put back when it ends.
struct Parked {
    real: Arc<dyn Backend>,
    /// Switched through the app's `SwitchBackend` (and its previous mode);
    /// `None` = the window's own backend was swapped (headless tests).
    via: Option<(Arc<SwitchBackend>, Mode)>,
    /// The real backend's events while the tutorial runs.
    events: Arc<Mutex<Vec<CoreEvent>>>,
    fake: Option<Arc<FakeBackend>>,
    provider: Option<Arc<dyn Provider>>,
    view: ViewMode,
    query: String,
    current: Option<LocalId>,
    editing: bool,
}

pub struct Tutorial {
    pub step: usize,
    /// The step's key was pressed; the card shows ✓ and moves on shortly.
    pub done: bool,
    /// Bumped on every step change (stale "move on" timers check it).
    generation: usize,
    /// The post and its text when a "type something" step began.
    typed_from: Option<(Option<LocalId>, String)>,
    parked: Parked,
}

impl Tutorial {
    pub fn current(&self) -> &'static steps::Step {
        &STEPS[self.step]
    }
}

fn same_backend(switch: &Arc<SwitchBackend>, backend: &Arc<dyn Backend>) -> bool {
    std::ptr::addr_eq(Arc::as_ptr(switch), Arc::as_ptr(backend))
}

impl MainView {
    // ------------------------------------------------------------ start / end

    pub(in crate::app) fn start_tutorial(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.onboarding.tutorial.is_some() {
            return;
        }
        self.onboarding.flow = None;
        self.sheet = None;
        self.ai_escape(window, cx);
        self.reading.sheet = None;
        self.leave_reading();

        let fake = (self.onboarding.make_fake)();
        let events = Arc::new(Mutex::new(Vec::new()));
        let switch = cx
            .try_global::<Connection>()
            .map(|c| c.switch.clone())
            .filter(|s| same_backend(s, &self.backend));
        let real = match &switch {
            Some(s) => s.current(),
            None => self.backend.clone(),
        };
        {
            let held = events.clone();
            real.set_event_sink(Box::new(move |e| {
                held.lock().unwrap_or_else(|p| p.into_inner()).push(e)
            }));
        }
        let via = match switch {
            Some(s) => {
                let mode = s.mode();
                s.switch(fake.clone(), Mode::Fake, || {});
                Some((s, mode))
            }
            None => {
                let sink = self.onboarding_ui_sink(window, cx);
                fake.set_event_sink(sink);
                self.backend = fake.clone();
                None
            }
        };
        crate::ai::init(cx);
        let provider = cx
            .global_mut::<crate::ai::AiGlobal>()
            .test_provider
            .replace(Arc::new(CannedAi));
        let parked = Parked {
            real,
            via,
            events,
            fake: self.fake.replace(fake),
            provider,
            view: self.studio.view,
            query: self.list.query().to_string(),
            current: self.current.as_ref().map(|c| c.local_id.clone()),
            editing: self.mode == EditMode::Edit,
        };
        self.base_url = self.backend.base_url();
        self.reading = crate::app::reading::State::new(&*self.backend, cx);
        self.current = None;
        self.onboarding.tutorial = Some(Tutorial {
            step: 0,
            done: false,
            generation: 0,
            typed_from: None,
            parked,
        });
        self.tutorial_enter(0, window, cx);
    }

    /// Back to the real backend, as the user left it.
    pub(in crate::app) fn finish_tutorial(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.onboarding.tutorial.take() else {
            return;
        };
        let p = t.parked;
        self.ai_escape(window, cx);
        self.sheet = None;
        self.reading.sheet = None;
        self.leave_reading();
        if cx.has_global::<crate::ai::AiGlobal>() {
            cx.global_mut::<crate::ai::AiGlobal>().test_provider = p.provider;
        }
        match p.via {
            Some((switch, mode)) => switch.switch(p.real.clone(), mode, || {}),
            None => {
                let sink = self.onboarding_ui_sink(window, cx);
                p.real.set_event_sink(sink);
                self.backend = p.real.clone();
            }
        }
        self.fake = p.fake;
        self.base_url = self.backend.base_url();
        self.reading = crate::app::reading::State::new(&*self.backend, cx);
        self.studio_set_view(p.view, cx);

        self.current = None;
        self.omni
            .update(cx, |s, cx| s.set_value(p.query.clone(), window, cx));
        let results = self.backend.search(&p.query);
        self.list.set_query(&p.query, results);
        let back = p.current.filter(|id| self.backend.item(id).is_some());
        if let Some(id) = &back {
            self.list.select(id);
        }
        self.load_selected(window, cx);
        match back {
            Some(id) if p.editing => self.open(&id, window, cx),
            _ => self.back_to_search(window, cx),
        }
        let held = std::mem::take(&mut *p.events.lock().unwrap_or_else(|e| e.into_inner()));
        for ev in held {
            if matches!(ev, CoreEvent::Conflict { .. } | CoreEvent::Error(_)) {
                self.on_core_event(ev, window, cx);
            }
        }
        self.show_toast(
            "Tour finished",
            Some(crate::keymap::hint("Back to your own posts · ⌘, › Help replays it").into()),
            cx,
        );
        cx.notify();
    }

    // ------------------------------------------------------------ steps

    pub(super) fn tutorial_enter(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.onboarding.tutorial.as_mut() else {
            return;
        };
        let i = i.min(STEPS.len() - 1);
        t.step = i;
        t.done = false;
        t.generation += 1;
        t.typed_from = None;
        let step = &STEPS[i];
        self.tutorial_setup(step.setup, window, cx);
        if step.accepts(Key::Typed) {
            let text = self.editor.read(cx).value().to_string();
            let id = self.current.as_ref().map(|c| c.local_id.clone());
            if let Some(t) = self.onboarding.tutorial.as_mut() {
                t.typed_from = Some((id, text));
            }
        }
        cx.notify();
    }

    pub(super) fn tutorial_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(step) = self.onboarding.tutorial.as_ref().map(|t| t.step) else {
            return;
        };
        if step + 1 >= STEPS.len() {
            self.finish_tutorial(window, cx);
        } else {
            self.tutorial_enter(step + 1, window, cx);
        }
    }

    pub(super) fn tutorial_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(step) = self.onboarding.tutorial.as_ref().map(|t| t.step)
            && step > 0
        {
            self.tutorial_enter(step - 1, window, cx);
        }
    }

    /// The user did `key`. If the step waits for it, show ✓ and move on
    /// after the step's pause (once things have settled, if it asks).
    pub(super) fn tutorial_key(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.onboarding.tutorial.as_mut() else {
            return;
        };
        let step = &STEPS[t.step];
        if t.done || !step.accepts(key) {
            return;
        }
        t.done = true;
        let (generation, at, pause, settle) = (t.generation, t.step, step.pause_ms, step.settle);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(pause))
                .await;
            loop {
                let moved = this.update_in(cx, |v, window, cx| {
                    match v.onboarding.tutorial.as_ref() {
                        Some(t) if t.generation == generation => {}
                        _ => return true, // ended, or the user moved on
                    }
                    if settle && v.tutorial_busy() {
                        return false;
                    }
                    if at + 1 >= STEPS.len() {
                        v.finish_tutorial(window, cx);
                    } else {
                        v.tutorial_enter(at + 1, window, cx);
                    }
                    true
                });
                match moved {
                    Ok(false) => {
                        cx.background_executor()
                            .timer(Duration::from_millis(150))
                            .await
                    }
                    _ => break,
                }
            }
        })
        .detach();
    }

    /// A sheet, an AI proposal or a generation is still on screen.
    fn tutorial_busy(&self) -> bool {
        self.sheet.is_some()
            || self.reading.sheet.is_some()
            || self.ai.has_overlay()
            || self.render_ai_status().is_some()
    }

    /// "Type something" steps: typing isn't an action, so watch the text.
    pub(super) fn tutorial_watch_typing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, from)) = self
            .onboarding
            .tutorial
            .as_ref()
            .filter(|t| !t.done)
            .and_then(|t| t.typed_from.clone())
        else {
            return;
        };
        // Only an edit of the same post counts, not the preview moving on.
        let same_post = self.current.as_ref().map(|c| &c.local_id) == id.as_ref();
        if self.mode == EditMode::Edit && same_post && self.editor.read(cx).value().as_ref() != from
        {
            self.tutorial_key(Key::Typed, window, cx);
        }
    }

    /// Hook: every action a step can wait for, seen on its way down (the
    /// capture phase), so the app still does what the key does.
    pub(super) fn tutorial_listeners(
        &self,
        d: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        if self.onboarding.tutorial.is_none() {
            return d;
        }
        macro_rules! on {
            ($d:expr, $action:ty, $key:expr) => {
                $d.capture_action(
                    cx.listener(|this, _: &$action, window, cx| {
                        this.tutorial_key($key, window, cx)
                    }),
                )
            };
        }
        let d = d.capture_action(cx.listener(|this, a: &Enter, window, cx| {
            if !a.secondary {
                this.tutorial_key(Key::Enter, window, cx)
            }
        }));
        let d = on!(d, MoveUp, Key::MoveSelection);
        let d = on!(d, MoveDown, Key::MoveSelection);
        let d = on!(d, ToggleKind, Key::ToggleKind);
        let d = on!(d, MakeDraft, Key::MakeDraft);
        let d = on!(d, Publish, Key::Publish);
        let d = on!(d, ViewWrite, Key::ViewWrite);
        let d = on!(d, ViewSplit, Key::ViewSplit);
        let d = on!(d, ViewStudio, Key::ViewStudio);
        let d = on!(d, crate::ai::AiGenerate, Key::AiGenerate);
        let d = on!(d, crate::ai::AiShorten, Key::AiShorten);
        let d = on!(d, crate::app::reading::ShowVersions, Key::ShowVersions);
        let d = on!(d, crate::app::reading::ShowReading, Key::ShowReading);
        on!(d, crate::app::reading::QuotePicker, Key::QuotePicker)
    }

    /// Close whatever the last step left open.
    fn tutorial_tidy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sheet.is_some() {
            self.sheet = None;
        }
        if self.ai.has_overlay() {
            self.ai_escape(window, cx);
        }
        self.reading.sheet = None;
        self.leave_reading();
    }

    fn tutorial_open(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let id = LocalId(id.into());
        if self.list.query() != "" {
            self.set_query_text("", window, cx);
        }
        self.open(&id, window, cx);
    }

    fn tutorial_setup(&mut self, setup: Setup, window: &mut Window, cx: &mut Context<Self>) {
        match setup {
            Setup::Nothing => {}
            Setup::Search => {
                self.tutorial_tidy(window, cx);
                self.studio_set_view(ViewMode::Write, cx);
                self.set_query_text("", window, cx);
                self.back_to_search(window, cx);
            }
            Setup::Query(q) => {
                self.tutorial_tidy(window, cx);
                self.studio_set_view(ViewMode::Write, cx);
                self.set_query_text(q, window, cx);
                self.mode = EditMode::Search;
                self.omni.update(cx, |s, cx| s.focus(window, cx));
            }
            Setup::EnsureOpen => {
                self.tutorial_tidy(window, cx);
                match self.current.as_ref().map(|c| c.local_id.clone()) {
                    Some(id) if self.mode == EditMode::Edit => self.open(&id, window, cx),
                    _ => self.tutorial_open(steps::DRAFT, window, cx),
                }
            }
            Setup::Scratch(text) => {
                self.tutorial_tidy(window, cx);
                self.set_query_text("", window, cx);
                match self.backend.create_scratch(Kind::Fragment, text) {
                    Ok(id) => {
                        let results = self.backend.search("");
                        self.list.set_query("", results);
                        self.current = None;
                        self.open(&id, window, cx);
                    }
                    Err(e) => {
                        self.show_toast(format!("Couldn't make a scratch note: {e}"), None, cx)
                    }
                }
            }
            Setup::Publishable => {
                self.tutorial_tidy(window, cx);
                let keep = self
                    .current
                    .as_ref()
                    .filter(|c| {
                        matches!(c.status, Status::Draft | Status::Scratch)
                            && !c.content_md.trim().is_empty()
                    })
                    .map(|c| c.local_id.clone());
                match keep {
                    Some(id) => self.open(&id, window, cx),
                    None => self.tutorial_open(steps::DRAFT, window, cx),
                }
            }
            Setup::PublishSheet => {
                if !matches!(self.sheet, Some(Sheet::Publish { .. })) {
                    self.tutorial_tidy(window, cx);
                    if self.current.is_none() {
                        self.tutorial_open(steps::DRAFT, window, cx);
                    }
                    self.publish(window, cx);
                }
            }
            Setup::StudioSample => {
                self.tutorial_tidy(window, cx);
                self.studio_set_view(ViewMode::Write, cx);
                let md = crate::fake::studio_sample();
                if let Ok(id) = self.backend.create_draft(Kind::Thread, &md) {
                    self.set_query_text("", window, cx);
                    self.current = None;
                    self.open(&id, window, cx);
                }
            }
            Setup::Tk(instruction) => {
                self.tutorial_tidy(window, cx);
                self.studio_set_view(ViewMode::Write, cx);
                self.tutorial_open(steps::DRAFT, window, cx);
                let old = self.editor.read(cx).value().to_string();
                let base = match old.find(" [TK]") {
                    Some(i) => old[..i].to_string(),
                    None => old.clone(),
                };
                let new = format!("{base} [TK]{instruction}[/TK]");
                let caret = new.len() - "[/TK]".len();
                self.splice_editor(&old, &new, Some(caret), window, cx);
            }
            Setup::Long(sentence) => {
                self.tutorial_tidy(window, cx);
                self.studio_set_view(ViewMode::Write, cx);
                self.tutorial_open(steps::DRAFT, window, cx);
                let old = self.editor.read(cx).value().to_string();
                let mut new = old.clone();
                while blyg_core::published_len(&new) <= 1100 {
                    new.push(' ');
                    new.push_str(sentence.trim());
                }
                let end = new.len();
                self.splice_editor(&old, &new, Some(end), window, cx);
            }
            Setup::Versioned => {
                self.tutorial_tidy(window, cx);
                self.tutorial_open(steps::VERSIONED, window, cx);
            }
            Setup::Posts => {
                self.tutorial_tidy(window, cx);
                self.back_to_search(window, cx);
            }
            Setup::Thread => {
                self.tutorial_tidy(window, cx);
                self.tutorial_open(steps::THREAD, window, cx);
            }
        }
    }

    // ------------------------------------------------------------ drawing

    /// Where a region is on screen: (x, y, w, h), from the same layout rules
    /// `render` uses.
    fn tutorial_region(&self, region: Region, window: &Window) -> Option<(f32, f32, f32, f32)> {
        let vp = window.viewport_size();
        let (w, h) = (f32::from(vp.width), f32::from(vp.height));
        let top = TITLEBAR_H;
        let body_top = top + OMNI_H;
        let body_h = (h - STATUS_H - body_top).max(0.);
        let view = self.studio.view;
        let (show_list, show_preview) = (view.list_visible(), view.preview_visible());
        let stacked = self.prefs.layout == crate::prefs::LayoutPref::Stacked;
        let list_w = (w * if show_preview { 0.24 } else { 0.38 }).round();
        let rest = if stacked || !show_list {
            w
        } else {
            w - list_w - 1.
        };
        let pane_w = if show_preview {
            (rest / 2.).floor()
        } else {
            rest
        };
        let list = if !show_list {
            None
        } else if stacked {
            Some((0., body_top, w, (h * 0.34).round()))
        } else {
            Some((0., body_top, list_w, body_h))
        };
        let editor = match (show_list, stacked, list) {
            (true, true, Some((_, _, _, lh))) => (0., body_top + lh, pane_w, body_h - lh),
            (true, false, _) => (list_w + 1., body_top, pane_w, body_h),
            _ => (0., body_top, pane_w, body_h),
        };
        Some(match region {
            Region::Whole => return None,
            Region::Omnibar => (0., top, w, OMNI_H),
            Region::Search => match list {
                Some((_, _, lw, lh)) if !stacked => (0., top, lw, OMNI_H + lh),
                Some((_, _, _, lh)) => (0., top, w, OMNI_H + lh),
                None => (0., top, w, OMNI_H),
            },
            Region::List => list?,
            Region::Editor => editor,
            Region::Counter => (0., h - STATUS_H, 230., STATUS_H),
            Region::ViewSwitcher => ((w - 318.).max(0.), 0., 312., top),
            Region::Sheet => ((w - 468.) / 2., top, 468., 132.),
        })
    }

    /// Where the step's ring goes, if anywhere. A sheet or picker the step
    /// opened sits on top of the panes: a ring around a pane would draw over
    /// it (only a Sheet step rings the sheet).
    pub(super) fn tutorial_ring(
        &self,
        region: Region,
        window: &Window,
    ) -> Option<(f32, f32, f32, f32)> {
        let covered = self.sheet.is_some()
            || self.reading.sheet.is_some()
            || self.ai.has_overlay()
            || self.profile_sheet_open();
        if covered && region != Region::Sheet {
            return None;
        }
        self.tutorial_region(region, window)
    }

    pub(super) fn render_tutorial(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let t = self.onboarding.tutorial.as_ref()?;
        let p = self.palette;
        let step = t.current();
        let (index, done) = (t.step, t.done);
        let last = index + 1 == STEPS.len();
        let buttons = super::show_buttons(cx);
        let hotkey = crate::prefs::hotkey_glyphs(&self.prefs.hotkey);
        let caption = crate::keymap::hint(step.caption).replace("{hotkey}", &hotkey);

        let ring = |(x, y, w, h): (f32, f32, f32, f32), id: &'static str, strong: bool| {
            div()
                .absolute()
                .left(px(x + 2.))
                .top(px(y + 2.))
                .w(px((w - 4.).max(8.)))
                .h(px((h - 4.).max(8.)))
                .rounded(px(8.))
                .border_2()
                .border_color(p.accent.opacity(if strong { 1.0 } else { 0.55 }))
                .bg(p.accent.opacity(0.035))
                .with_animation(
                    id,
                    Animation::new(Duration::from_millis(1600)).repeat(),
                    |d, t| d.opacity(1.0 - 0.45 * (1.0 - (2.0 * t - 1.0).abs())),
                )
        };
        let kbd = |k: String| {
            div()
                .px(px(7.))
                .py(px(1.))
                .min_w(px(22.))
                .flex()
                .justify_center()
                .rounded(px(5.))
                .border_1()
                .border_b_2()
                .border_color(p.line)
                .text_color(p.ink)
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(12.))
                .child(k)
        };
        let link = |id: &'static str, label: String| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(5.))
                .px(px(9.))
                .py(px(3.))
                .rounded_full()
                .border_1()
                .border_color(p.line)
                .cursor_pointer()
                .hover(|s| s.border_color(p.accent).text_color(p.ink))
                .child(label)
        };

        let key_row = if done {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_color(p.green)
                .font_weight(FontWeight::MEDIUM)
                .child("✓ That's it")
                .into_any_element()
        } else if step.keys.is_empty() {
            div().into_any_element()
        } else {
            div()
                .flex()
                .items_center()
                .gap(px(7.))
                .text_color(p.muted)
                .child("Press")
                .child(kbd(crate::keymap::hint(step.key_label).to_string()))
                .into_any_element()
        };

        let card = div()
            .id("tutorial-card")
            .occlude()
            .absolute()
            .left(px(14.))
            .bottom(px(STATUS_H + 14.))
            .w(px(380.))
            .max_w(relative(0.9))
            .bg(p.bg)
            .border_1()
            .border_color(p.line)
            .rounded(px(12.))
            .shadow(vec![BoxShadow {
                color: p.shadow,
                offset: point(px(0.), px(14.)),
                blur_radius: px(36.),
                spread_radius: px(-8.),
                inset: false,
            }])
            .px(px(16.))
            .py(px(13.))
            .font_family(SharedString::from(self.prefs.ui().family))
            .text_size(px(12.5))
            .child(
                div()
                    .flex()
                    .items_center()
                    .text_size(px(10.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.muted)
                    .child(format!(
                        "TOUR · {} OF {} · SAMPLE DATA",
                        index + 1,
                        STEPS.len()
                    ))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("tutorial-end")
                            .cursor_pointer()
                            .hover(|s| s.text_color(p.ink))
                            .child("End tour ✕")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.finish_tutorial(window, cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .mt(px(5.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(15.))
                    .child(crate::keymap::hint(step.title)),
            )
            .child(
                div()
                    .mt(px(4.))
                    .text_color(p.ink.opacity(0.8))
                    .line_height(relative(1.45))
                    .child(caption),
            )
            .child(div().mt(px(9.)).child(key_row))
            .when_some(step.button.filter(|_| buttons), |d, b| {
                d.child(
                    div()
                        .mt(px(5.))
                        .text_size(px(11.5))
                        .text_color(p.muted)
                        .child(format!("or click “{b}” in the toolbar ↑")),
                )
            })
            .child(
                div()
                    .mt(px(12.))
                    .pt(px(9.))
                    .border_t_1()
                    .border_color(p.line)
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(self.render_tutorial_checkbox("tutorial-on-launch", cx))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(11.5))
                            .text_color(p.muted)
                            .child(div().flex_1())
                            .when(index > 0, |d| {
                                d.child(
                                    link(
                                        "tutorial-back",
                                        crate::keymap::hint("‹ Back  ⌥⌘←").into(),
                                    )
                                    .on_click(cx.listener(
                                        |this, _, window, cx| this.tutorial_back(window, cx),
                                    )),
                                )
                            })
                            .child(
                                link(
                                    "tutorial-next",
                                    if last {
                                        crate::keymap::hint("Finish  ⌥⌘→").into()
                                    } else {
                                        crate::keymap::hint("Next ›  ⌥⌘→").into()
                                    },
                                )
                                .when(last, |d| d.border_color(p.accent).text_color(p.accent))
                                .on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.tutorial_next(window, cx)
                                    }),
                                ),
                            ),
                    ),
            );

        let rect = self.tutorial_ring(step.region, window);
        let toolbar = (buttons && step.button.is_some()).then(|| {
            let w = f32::from(window.viewport_size().width);
            (90., 0., (w - 420.).max(60.), TITLEBAR_H)
        });
        Some(
            div()
                .absolute()
                .inset_0()
                .children(rect.map(|r| ring(r, "tutorial-ring", true)))
                .children(toolbar.map(|r| ring(r, "tutorial-toolbar", false)))
                .child(card)
                .into_any_element(),
        )
    }
}

// ================================================================ canned AI

/// The tutorial's stand-in for an AI provider: fixed words, no network,
/// no CLI.
struct CannedAi;

impl Provider for CannedAi {
    fn kind(&self) -> ProviderKind {
        ProviderKind::LocalCodex
    }

    fn generate(
        &self,
        req: GenRequest,
        on_delta: &mut dyn FnMut(&str),
    ) -> blyg_ai::Result<GenResult> {
        std::thread::sleep(Duration::from_millis(500));
        req.cancel.check()?;
        let text = if req.user.contains("shorten to fit") || req.system.contains("shorten to fit") {
            "A lighthouse keeper's log is mostly weather, and that is the point: wind, swell and \
             visibility, hour after hour, in the same hand."
        } else {
            "Fog rolls in the way a good sentence does: slowly, then all at once."
        };
        on_delta(text);
        Ok(GenResult {
            text: text.to_string(),
            model: "tutorial-demo".into(),
        })
    }
}

// ================================================================ snapshots

/// `BLYGGER_SNAPSHOT=<file.png>`: once the demo has played, render the
/// window's last frame to a PNG and quit (same as the reading demos; only
/// in a `--cfg blygger_snap` build with `gpui-kit/test-support`).
pub(super) fn snapshot_later(window: &mut Window, cx: &mut Context<MainView>) {
    let Ok(path) = std::env::var("BLYGGER_SNAPSHOT") else {
        return;
    };
    cx.spawn_in(window, async move |_, cx| {
        cx.background_executor()
            .timer(Duration::from_millis(900))
            .await;
        for _ in 0..2 {
            let _ = cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
        }
        let _ = cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            snap::save_frame(window, &path);
            cx.quit();
        });
    })
    .detach();
}

#[allow(unexpected_cfgs)]
mod snap {
    use gpui_kit::Window;

    #[cfg(blygger_snap)]
    pub fn save_frame(window: &mut Window, path: &str) {
        match window.render_to_image() {
            Ok(img) => match img.save(path) {
                Ok(()) => println!("snapshot {path}"),
                Err(e) => eprintln!("snapshot failed: {e}"),
            },
            Err(e) => eprintln!("snapshot failed: {e}"),
        }
    }

    #[cfg(not(blygger_snap))]
    pub fn save_frame(_: &mut Window, path: &str) {
        eprintln!("BLYGGER_SNAPSHOT={path}: this build can't render snapshots");
    }
}
