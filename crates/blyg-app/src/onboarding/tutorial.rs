//! The interactive tutorial: an overlay on the real window that runs the
//! script in `steps.rs` on a fresh FakeBackend.
//!
//! Starting it *parks* the real backend: the app's `SwitchBackend` is
//! switched to the tutorial's FakeBackend (so quick capture and the status
//! bar follow), and the real backend's events are held back. Finishing
//! puts the real backend back, replays any held conflict or error, and
//! restores the query, selection and view mode, the reading mode (Stream or
//! Reader), the notes drawer's note and the browser pane's page. ⌘G / ⇧⌘G
//! get a canned provider meanwhile, so no AI is ever called, and the browser
//! step shows a blank sample page, so no web page is loaded.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use blyg_ai::{GenRequest, GenResult, Provider, ProviderKind};
use blyg_core::{Backend, CoreEvent, Kind, LocalId, Status};
use gpui_kit::base::input::{Enter, MoveDown, MoveUp};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::steps::{self, Key, Region, STEPS, Setup};
use crate::app::reading::View;
use crate::app::reading::stream_vm::{self, ReadMode};
use crate::app::scratch::MakeDraft;
use crate::app::studio::{ViewMode, ViewSplit, ViewStudio, ViewWrite};
use crate::app::{
    MainView, Mode as EditMode, OMNI_H, Publish, STATUS_H, Sheet, TITLEBAR_H, ToggleKind,
};
use crate::connection::{Connection, Mode, SwitchBackend};
use crate::fake::FakeBackend;
use crate::theme::Rule; // --- themes --- dividers

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
    /// Stream or Reader (the tour switches it; it's remembered in state.json).
    read_mode: ReadMode,
    /// The notes drawer's note (the tour's drawer writes to the sample data).
    notes: Option<crate::app::notes::NotesPark>,
}

pub struct Tutorial {
    pub step: usize,
    /// The step's key was pressed; the card shows ✓ and moves on shortly.
    pub done: bool,
    /// Bumped on every step change (stale "move on" timers check it).
    generation: usize,
    /// The post and its text when a "type something" step began.
    typed_from: Option<(Option<LocalId>, String)>,
    /// What `tutorial_observed` saw when the step began: only a change
    /// after that counts.
    seen: Vec<Key>,
    /// Frames asked for while the step's region waits for its layout.
    retries: u8,
    /// Stay on the step after its key (`BLYGGER_DEMO=tut-<id>+` snapshots).
    hold: bool,
    /// After an update: what changed, shown before the first step until
    /// Take the tour, Start at what's new, or Skip.
    pub news: Vec<&'static super::whats_new::Release>,
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
        let read_mode = self.reading.mode;
        let notes = self.notes_tour_park(cx);
        self.browser_tour_park(cx);
        self.close_profile(window, cx);

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
            read_mode,
            notes: Some(notes),
        };
        self.base_url = self.backend.base_url();
        self.reading = crate::app::reading::State::new(&*self.backend, cx);
        self.current = None;
        self.onboarding.tutorial = Some(Tutorial {
            step: 0,
            done: false,
            generation: 0,
            typed_from: None,
            seen: Vec::new(),
            retries: 0,
            hold: false,
            news: Vec::new(),
            parked,
        });
        self.tutorial_enter(0, window, cx);
    }

    /// After an update: the tutorial, opening on what changed.
    pub(in crate::app) fn start_whats_new(
        &mut self,
        news: Vec<&'static super::whats_new::Release>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_tutorial(window, cx);
        if let Some(t) = self.onboarding.tutorial.as_mut() {
            t.news = news;
        }
        // The card has the keyboard: ⏎ what's new, esc skips.
        window.focus(&self.onboarding.focus, cx);
        cx.notify();
    }

    /// Leave the what's-new card for the tour, at its start or at the
    /// first new step.
    fn whats_new_go(&mut self, at_new: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.onboarding.tutorial.as_mut() else {
            return;
        };
        let first = t.news.iter().rev().find_map(|r| r.first_step);
        t.news.clear();
        let i = first
            .filter(|_| at_new)
            .and_then(|id| STEPS.iter().position(|s| s.id == id))
            .unwrap_or(0);
        self.tutorial_enter(i, window, cx);
    }

    /// Back to the real backend, as the user left it.
    pub(in crate::app) fn finish_tutorial(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.onboarding.tutorial.take() else {
            return;
        };
        let mut p = t.parked;
        self.ai_escape(window, cx);
        self.sheet = None;
        self.reading.sheet = None;
        self.leave_reading();
        self.close_profile(window, cx);
        if let Some(notes) = p.notes.take() {
            self.notes_tour_restore(notes, cx);
        }
        self.browser_tour_end(cx);
        // The reading mode as it was (the new reading state loads it).
        let data_dir = cx.try_global::<Connection>().map(|c| c.data_dir.clone());
        stream_vm::save_mode(data_dir.as_deref(), p.read_mode);
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
        t.retries = 0;
        let step = &STEPS[i];
        self.tutorial_setup(step.setup, window, cx);
        let seen = self.tutorial_observed(cx);
        if let Some(t) = self.onboarding.tutorial.as_mut() {
            t.seen = seen;
        }
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
        if self
            .onboarding
            .tutorial
            .as_ref()
            .is_some_and(|t| !t.news.is_empty())
        {
            // ⌥⌘→: the primary choice, what's new (else the whole tour).
            return self.whats_new_go(true, window, cx);
        }
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
    /// after the step's pause (once things have settled, if it asks), or,
    /// for a step that `stay`s, leave the moving on to Next.
    pub(super) fn tutorial_key(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.onboarding.tutorial.as_mut() else {
            return;
        };
        let step = &STEPS[t.step];
        if t.done || !step.accepts(key) || !t.news.is_empty() {
            return;
        }
        t.done = true;
        let (generation, at, pause, settle) = (t.generation, t.step, step.pause_ms, step.settle);
        cx.notify();
        if step.stay {
            return; // ✓, and Next when they're ready
        }
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
                    if (settle && v.tutorial_busy())
                        || v.onboarding.tutorial.as_ref().is_some_and(|t| t.hold)
                    {
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

    /// Keys that are states rather than actions (a pane that opened, a
    /// popup that showed): what's true now.
    fn tutorial_observed(&self, cx: &App) -> Vec<Key> {
        let reading = self.reading.view == View::Reading;
        let reader = reading && self.reading.mode == ReadMode::Reader;
        let opened = self.reading.opened.as_ref().filter(|_| reading);
        let assist = self.assist.read(cx);
        [
            (Key::ReadMore, opened.is_some() && !reader),
            (
                Key::OpenOriginal,
                opened.is_some_and(|o| o.item.remote_id == steps::QUOTED),
            ),
            (Key::OpenProfile, self.profile_sheet_open()),
            (Key::ReaderMode, reader),
            (Key::Notes, self.notes.open),
            (Key::CloseBrowser, !self.browser.open),
            (Key::Mention, assist.mention_open()),
            (Key::SpellMenu, assist.menu_open()),
            (Key::Lineage, self.lineage_open()),
            (Key::LineageRing, self.lineage_ring_open()),
        ]
        .into_iter()
        .filter_map(|(k, on)| on.then_some(k))
        .collect()
    }

    /// Per frame: a state key the step waits for that has come true since
    /// the step began.
    pub(super) fn tutorial_watch_observed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = self.tutorial_observed(cx);
        let Some(t) = self.onboarding.tutorial.as_mut().filter(|t| !t.done) else {
            return;
        };
        let step = &STEPS[t.step];
        let hit = now
            .iter()
            .copied()
            .find(|k| step.accepts(*k) && !t.seen.contains(k));
        // Something that went away and comes back counts again.
        t.seen.retain(|k| now.contains(k));
        if let Some(k) = hit {
            self.tutorial_key(k, window, cx);
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
        self.reading.opened = None;
        if self.notes.drawn() {
            self.notes_tour_hide(cx);
        }
        self.browser_tour_hide(cx);
        if self.profile_sheet_open() {
            self.close_profile(window, cx);
        }
    }

    /// Reading, in `mode`, nothing open, the search cleared.
    fn tutorial_reading(&mut self, mode: ReadMode, window: &mut Window, cx: &mut Context<Self>) {
        self.tutorial_tidy(window, cx);
        if !self.reading.query.is_empty() {
            self.set_reading_query("", window, cx);
        }
        self.reading.sel = None;
        // --- reader folders --- All, as a fresh Reader starts.
        self.reading.source = Default::default();
        self.reading.sticky.clear();
        self.set_read_mode(mode, window, cx);
        self.reading.opened = None;
    }

    /// The sample reading post `remote_id`'s key.
    fn tutorial_reading_key(&self, remote_id: &str) -> Option<(String, String)> {
        self.reading
            .rows
            .iter()
            .find(|r| r.remote_id == remote_id)
            .map(crate::app::reading::vm::key)
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
            Setup::Mentions(text) => {
                self.tutorial_tidy(window, cx);
                self.studio_set_view(ViewMode::Write, cx);
                self.tutorial_open(steps::DRAFT, window, cx);
                let old = self.editor.read(cx).value().to_string();
                let base = match old.find(text) {
                    Some(i) => old[..i].trim_end().to_string(),
                    None => old.trim_end().to_string(),
                };
                let new = format!("{base}\n\n{text}");
                let end = new.len();
                self.splice_editor(&old, &new, Some(end), window, cx);
            }
            Setup::Stream => {
                self.tutorial_reading(ReadMode::Stream, window, cx);
                self.stream_move(1, window, cx);
            }
            Setup::StreamQuote => {
                self.tutorial_reading(ReadMode::Stream, window, cx);
                let ix = self
                    .reading
                    .shown_rows()
                    .position(|r| r.remote_id == steps::QUOTING);
                match ix {
                    Some(ix) => self.stream_select(ix, window, cx),
                    None => self.stream_move(1, window, cx),
                }
            }
            Setup::Lineage => {
                self.tutorial_setup(Setup::StreamQuote, window, cx);
                let at = self
                    .reading
                    .rows
                    .iter()
                    .find(|r| r.remote_id == steps::QUOTING)
                    .map(|r| r.origin.clone());
                if let Some(origin) = at {
                    self.open_lineage(origin, steps::QUOTING.into(), window, cx);
                }
            }
            Setup::Reader => {
                self.tutorial_reading(ReadMode::Reader, window, cx);
                if let Some(key) = self.tutorial_reading_key(steps::QUOTED) {
                    self.open_reading(key, window, cx);
                }
            }
            Setup::Browser => {
                self.tutorial_setup(Setup::Reader, window, cx);
                self.browser_tour_open(window, cx);
            }
        }
    }

    /// `BLYGGER_DEMO=tut-<id>+`: what the step's key does, the way a click
    /// or a key would (screenshots of the step after its key).
    pub(super) fn tutorial_demo_act(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(t) = self.onboarding.tutorial.as_mut() {
            t.hold = true;
        }
        match id {
            "stream" => self.stream_open_selected(window, cx),
            "original" => {
                let origin = self
                    .reading
                    .rows
                    .iter()
                    .find(|r| r.remote_id == steps::QUOTED)
                    .map(|r| r.origin.clone());
                if let Some(origin) = origin {
                    self.open_original(origin, steps::QUOTED.into(), Some(1), window, cx);
                }
            }
            "reader" => self.set_read_mode(ReadMode::Reader, window, cx),
            "notes" => self.open_notes(window, cx),
            "browser" => self.close_browser(window, cx),
            "mentions" => self.demo_type("@", window, cx),
            "quotes" => self.open_quote_picker(window, cx),
            "lineage" => self.toggle_lineage(window, cx),
            "ring" => self.lineage_open_ring(cx),
            _ => {}
        }
    }

    // ------------------------------------------------------------ drawing

    /// Where a region is on screen: (x, y, w, h), from the same layout rules
    /// `render` uses.
    pub(super) fn tutorial_region(&self, region: Region, window: &Window) -> Option<Rect> {
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
            Region::Sheet => ((w - 468.) / 2., top, 468., 132.),
            Region::Stream
            | Region::StreamPost
            | Region::Reader
            | Region::ReaderPost
            | Region::BrowserChrome => return self.tutorial_reading_region(region, w, h),
        })
    }

    /// The reading screen's regions (the same layout rules as
    /// `render_reading_screen`, `render_stream_body` and the three panes).
    fn tutorial_reading_region(&self, region: Region, w: f32, h: f32) -> Option<Rect> {
        if region == Region::BrowserChrome {
            return self.browser_rects(w, h).map(|[_, chrome]| chrome);
        }
        if self.reading.view != View::Reading || !self.reading.available {
            return None;
        }
        // Below the screen header, above the status bar.
        let top = TITLEBAR_H + OMNI_H;
        let body_h = (h - STATUS_H - top).max(0.);
        let stream = self.reading.mode == ReadMode::Stream;
        // The Stream | Reader toggle, at the header's right end.
        let toggle = (
            (w - 14. - TOGGLE_W - 4.).max(0.),
            TITLEBAR_H + 4.,
            TOGGLE_W + 8.,
            OMNI_H - 8.,
        );
        let sources_w = if self.reading.sources_open {
            self.reading.sources_w
        } else {
            0.
        };
        match region {
            Region::Stream if stream => {
                let pane = self
                    .reading
                    .opened
                    .as_ref()
                    .map_or(0., |_| stream_pane_w(w));
                Some((0., top, w - pane, body_h))
            }
            Region::StreamPost if stream => {
                let list = &self.reading.stream.list;
                let ix = self
                    .reading
                    .sel
                    .as_ref()
                    .and_then(|k| self.reading.shown_pos(k))?;
                let b = list.bounds_for_item(ix)?;
                let vp = list.viewport_bounds();
                let y0 = f32::from(b.top().max(vp.top()));
                let y1 = f32::from(b.bottom().min(vp.bottom()));
                (y1 - y0 > 8.).then(|| (f32::from(b.left()), y0, f32::from(b.size.width), y1 - y0))
            }
            Region::Reader if stream || !self.reading.sources_open => Some(toggle),
            Region::Reader => Some((0., top, sources_w, body_h)),
            Region::ReaderPost if !stream => {
                let list_w = w * if self.reading.sources_open { 0.3 } else { 0.38 };
                let x = (sources_w + list_w + 1.).round();
                Some((x, top, (w - x).max(0.), body_h))
            }
            _ => None,
        }
    }

    /// What covers the panes right now: the browser pane, the notes drawer
    /// and the stream's side pane (a ring around a pane under one of them
    /// would draw over it).
    fn tutorial_overlays(&self, region: Region, w: f32, h: f32) -> Vec<Rect> {
        let mut v = Vec::new();
        if region != Region::BrowserChrome
            && let Some([pane, _]) = self.browser_rects(w, h)
        {
            v.push(pane);
        }
        if self.notes.drawn() {
            let nw = crate::app::notes::WIDTH;
            v.push(((w - nw).max(0.), TITLEBAR_H, nw, h - TITLEBAR_H));
        }
        if self.reading.view == View::Reading
            && self.reading.mode == ReadMode::Stream
            && self.reading.opened.is_some()
        {
            let pw = stream_pane_w(w);
            v.push((
                w - pw,
                TITLEBAR_H + OMNI_H,
                pw,
                h - STATUS_H - TITLEBAR_H - OMNI_H,
            ));
        }
        v
    }

    /// Where the step's ring goes, if anywhere. A sheet or picker the step
    /// opened sits on top of the panes: a ring around a pane would draw over
    /// it (only a Sheet step rings the sheet). So do the browser pane, the
    /// notes drawer and the stream's side pane wherever they cover the
    /// region (only the browser step rings the browser pane).
    pub(super) fn tutorial_ring(
        &self,
        region: Region,
        window: &Window,
        cx: &App,
    ) -> Option<(f32, f32, f32, f32)> {
        let assist = self.assist.read(cx);
        let covered = self.sheet.is_some()
            || self.reading.sheet.is_some()
            || self.ai.has_overlay()
            || self.profile_sheet_open()
            // The editor's @-mention popup and spelling menu.
            || assist.mention_open()
            || assist.menu_open();
        if covered && region != Region::Sheet {
            return None;
        }
        let rect = self.tutorial_region(region, window)?;
        // The browser pane, the notes drawer and the stream's side pane
        // sit over the panes too (only the browser step rings its pane).
        let vp = window.viewport_size();
        let (w, h) = (f32::from(vp.width), f32::from(vp.height));
        if self
            .tutorial_overlays(region, w, h)
            .iter()
            .any(|o| overlaps(rect, *o))
        {
            return None;
        }
        Some(rect)
    }

    pub(super) fn render_tutorial(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let t = self.onboarding.tutorial.as_ref()?;
        if !t.news.is_empty() {
            return Some(self.render_whats_new(cx));
        }
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
                .border_color(p.edge())
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
                .border_color(p.edge())
                .cursor_pointer()
                .hover(|s| s.border_color(p.accent).text_color(p.ink))
                .child(label)
        };

        let key_row = if done {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_color(p.green_text())
                .font_weight(FontWeight::MEDIUM)
                .child("✓ That's it")
                .when(step.stay, |d| {
                    d.child(
                        div()
                            .text_color(p.muted)
                            .font_weight(FontWeight::NORMAL)
                            .child("Look around, then Next ›"),
                    )
                })
                .into_any_element()
        } else if step.keys.is_empty() {
            div().into_any_element()
        } else {
            div()
                .flex()
                .items_center()
                .gap(px(7.))
                .text_color(p.muted)
                // "Press ⌘T", but "Your turn: click" for what isn't a key.
                .child(if matches!(step.key_label, "type" | "click") {
                    "Your turn:"
                } else {
                    "Press"
                })
                .child(kbd(crate::keymap::hint(step.key_label).to_string()))
                .into_any_element()
        };

        let card = div()
            .id("tutorial-card")
            .occlude()
            .absolute()
            // Clear of the lineage sheet's responses row.
            .map(|d| {
                if self.lineage_open() {
                    d.right(px(14.))
                } else {
                    d.left(px(14.))
                }
            })
            .bottom(px(STATUS_H + 14.))
            .w(px(380.))
            .max_w(relative(0.9))
            // --- themes --- the theme's card.
            .map(|d| crate::theme_ext::card(d, &self.theme))
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
                    .rule_t(&p)
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
                                .when(last || (done && step.stay), |d| {
                                    d.border_color(p.accent).text_color(p.accent_text())
                                })
                                .on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.tutorial_next(window, cx)
                                    }),
                                ),
                            ),
                    ),
            );

        let rect = self.tutorial_ring(step.region, window, cx);
        // The stream's post has bounds only once the list has laid it out:
        // ask for a few more frames.
        if rect.is_none()
            && step.region == Region::StreamPost
            && let Some(t) = self.onboarding.tutorial.as_mut()
            && t.retries < 30
        {
            t.retries += 1;
            window.request_animation_frame();
        }
        // The matching toolbar button, a little larger than the button.
        let toolbar = step
            .button
            .filter(|_| buttons)
            .and_then(|b| self.toolbar_button_rect(b, window, cx))
            .map(|(x, y, w, h)| (x - 4., y - 4., w + 8., h + 8.));
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

impl MainView {
    /// The what's-new card, centred: what changed, then the choices.
    fn render_whats_new(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        let Some(t) = self.onboarding.tutorial.as_ref() else {
            return div().into_any_element();
        };
        let version = t.news.first().map_or("", |r| r.version);
        let has_new_step = t.news.iter().any(|r| r.first_step.is_some());
        let button = |id: &'static str, label: &'static str, strong: bool| {
            div()
                .id(id)
                .px(px(11.))
                .py(px(5.))
                .rounded_full()
                .border_1()
                .border_color(if strong { p.accent } else { p.edge() })
                .text_color(if strong { p.accent_text() } else { p.ink })
                .cursor_pointer()
                .hover(|s| s.border_color(p.accent))
                .child(label)
        };
        let card = div()
            .id("whats-new")
            .track_focus(&self.onboarding.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                match ev.keystroke.key.as_str() {
                    "escape" => this.finish_tutorial(window, cx),
                    "enter" => this.whats_new_go(true, window, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .occlude()
            .w(px(480.))
            .max_w(relative(0.92))
            .map(|d| crate::theme_ext::card(d, &self.theme))
            .px(px(20.))
            .py(px(16.))
            .font_family(SharedString::from(self.prefs.ui().family))
            .text_size(px(13.))
            .child(
                div()
                    .text_size(px(10.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.muted)
                    .child("UPDATED"),
            )
            .child(
                div()
                    .mt(px(4.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(17.))
                    .child(format!("What's new in Burrow {version}")),
            )
            .child(
                div()
                    .mt(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .line_height(relative(1.45))
                    .text_color(p.ink.opacity(0.85))
                    .children(t.news.iter().flat_map(|r| r.items.iter()).map(|line| {
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(div().flex_none().text_color(p.accent).child("•"))
                            .child(div().flex_1().min_w_0().child(*line))
                    })),
            )
            .child(
                div()
                    .mt(px(14.))
                    .pt(px(10.))
                    .rule_t(&p)
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(12.))
                    .child(
                        button("whats-new-skip", "Skip tutorial  esc", false).on_click(
                            cx.listener(|this, _, window, cx| this.finish_tutorial(window, cx)),
                        ),
                    )
                    .child(div().flex_1())
                    .child(
                        button("whats-new-tour", "Take the whole tour", false).on_click(
                            cx.listener(|this, _, window, cx| this.whats_new_go(false, window, cx)),
                        ),
                    )
                    .when(has_new_step, |d| {
                        d.child(
                            button("whats-new-start", "Show me what's new  ⏎", true).on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.whats_new_go(true, window, cx)
                                }),
                            ),
                        )
                    }),
            );
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(p.shadow.opacity(0.25))
            .child(card)
            .into_any_element()
    }
}

/// (x, y, w, h) in window pixels.
type Rect = (f32, f32, f32, f32);

/// The Stream | Reader toggle's width (two segments, 11.5 px Inter).
const TOGGLE_W: f32 = 124.;

/// The stream's side pane: 54% of the window (`render_stream_body`).
fn stream_pane_w(w: f32) -> f32 {
    w * 0.54
}

/// The two rects share more than an edge.
fn overlaps(a: Rect, b: Rect) -> bool {
    let (ax1, ay1) = (a.0 + a.2, a.1 + a.3);
    let (bx1, by1) = (b.0 + b.2, b.1 + b.3);
    ax1.min(bx1) - a.0.max(b.0) > 1. && ay1.min(by1) - a.1.max(b.1) > 1.
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
            content_md: None,
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
