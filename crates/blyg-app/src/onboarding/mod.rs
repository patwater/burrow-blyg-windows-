//! First-run onboarding and the interactive tutorial.
//!
//! Onboarding is a card over the main window, shown on the first launch
//! (never onboarded, or no `blyg-url`): Welcome → Connect your blyg (the
//! existing Connect sheet, or sample data) → AI (optional) → Buttons or
//! keyboard? → the tutorial. esc or Skip ends it at any step; it never
//! blocks the app. "Onboarded" is app state (`state.json`), not config.
//!
//! The tutorial (`tutorial.rs`, script in `steps.rs`) walks through the
//! real window on a fresh FakeBackend, so the user's own blyg is never
//! touched while it runs.
//!
//! A child module of `app` (declared there with `#[path]`) so it can reach
//! the window's state; `app.rs` carries only small `// --- onboarding ---`
//! hooks.

mod steps;
mod tutorial;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use blyg_ai::{AccountRow, ProviderKind};
use blyg_core::config::Change;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{MainView, Sheet};
use crate::ai::settings as ais;
use crate::fake::FakeBackend;

pub use tutorial::Tutorial;

gpui_kit::actions!(
    blygger_onboarding,
    [ShowTutorial, TutorialNext, TutorialBack]
);

/// The literal config key for the toolbar (added to keys.rs by the toolbar
/// work; written through the config edit module either way).
pub const SHOW_BUTTONS: &str = "show-buttons";
pub const TUTORIAL_ON_LAUNCH: &str = "tutorial-on-launch";

/// Onboarding's steps, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowStep {
    Welcome,
    Connect,
    Ai,
    Buttons,
    Tutorial,
}

impl FlowStep {
    pub const ALL: [FlowStep; 5] = [
        FlowStep::Welcome,
        FlowStep::Connect,
        FlowStep::Ai,
        FlowStep::Buttons,
        FlowStep::Tutorial,
    ];

    fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    fn next(self) -> Option<FlowStep> {
        Self::ALL.get(self.index() + 1).copied()
    }

    fn prev(self) -> Option<FlowStep> {
        self.index().checked_sub(1).map(|i| Self::ALL[i])
    }
}

pub struct Flow {
    pub step: FlowStep,
    /// The Connect sheet is up (the card waits behind it).
    connecting: bool,
    /// Settings › AI is up (ditto).
    in_ai_settings: bool,
    ai_rows: Vec<AccountRow>,
    /// A failed write, shown on the card.
    message: Option<String>,
}

/// The window's onboarding + tutorial state (a field of `MainView`).
pub struct State {
    pub focus: FocusHandle,
    pub flow: Option<Flow>,
    pub tutorial: Option<Tutorial>,
    /// "Onboarded" when there's no data dir to keep it in (headless tests).
    onboarded_here: bool,
    /// The tutorial's sample data (instant timing in tests).
    pub(crate) make_fake: fn() -> Arc<FakeBackend>,
}

impl State {
    pub fn new(cx: &mut App) -> State {
        State {
            focus: cx.focus_handle(),
            flow: None,
            tutorial: None,
            onboarded_here: false,
            #[cfg(not(test))]
            make_fake: || Arc::new(FakeBackend::new()),
            #[cfg(test)]
            make_fake: || {
                Arc::new(
                    FakeBackend::with_timing(crate::fake::Timing::instant()).without_media_cache(),
                )
            },
        }
    }
}

fn data_dir(cx: &App) -> Option<std::path::PathBuf> {
    cx.try_global::<crate::connection::Connection>()
        .map(|c| c.data_dir.clone())
}

/// `show-buttons`, default true (read raw, so it works before keys.rs
/// knows the key).
pub fn show_buttons(cx: &App) -> bool {
    let store = &crate::settings::get(cx).store;
    let raw = store
        .config()
        .get(SHOW_BUTTONS)
        .map(str::to_string)
        .or_else(|| {
            store
                .loaded()
                .entries
                .iter()
                .rev()
                .find(|e| e.key == SHOW_BUTTONS)
                .map(|e| e.value.trim().to_ascii_lowercase())
        });
    raw.as_deref() != Some("false")
}

pub fn tutorial_on_launch(cx: &App) -> bool {
    crate::settings::get(cx).store.config().tutorial_on_launch()
}

impl MainView {
    // ------------------------------------------------------------ hooks

    /// Hook (MainView::new): onboarding on the first launch, else the
    /// tutorial if `tutorial-on-launch`. True when something was shown.
    /// Scripted demo runs (`BLYGGER_DEMO`) decide for themselves.
    pub(super) fn onboarding_launch(
        &mut self,
        has_blyg: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if std::env::var_os("BLYGGER_DEMO").is_some() {
            return false;
        }
        // Without a data dir (headless tests) there's no first-run memory:
        // those keep the plain Connect sheet.
        if let Some(dir) = data_dir(cx) {
            let onboarded = blyg_core::state::AppState::load(&dir).onboarded;
            // Already connected (e.g. set up before onboarding existed):
            // don't walk them through it; Settings › Help can replay it.
            if !onboarded && has_blyg {
                let _ = blyg_core::state::AppState::update(&dir, |s| s.onboarded = true);
            } else if !has_blyg {
                self.open_onboarding(FlowStep::Welcome, window, cx);
                return true;
            }
        }
        if tutorial_on_launch(cx) {
            self.start_tutorial(window, cx);
            return true;
        }
        false
    }

    /// Hook (top of render): keep the flow in step with the sheets it
    /// waits on, and draw the card or the tutorial overlay.
    pub(super) fn onboarding_frame(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.onboarding_sync(window, cx);
        if self.onboarding.tutorial.is_some() {
            self.tutorial_watch_typing(window, cx);
            return self.render_tutorial(window, cx);
        }
        self.render_onboarding(cx)
    }

    /// Hook: the root element's action handlers.
    pub(super) fn onboarding_actions(
        &self,
        d: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        self.tutorial_listeners(d, cx)
            .on_action(
                cx.listener(|this, _: &ShowTutorial, window, cx| this.start_tutorial(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &TutorialNext, window, cx| this.tutorial_next(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &TutorialBack, window, cx| this.tutorial_back(window, cx)),
            )
    }

    // ------------------------------------------------------------ flow

    pub(super) fn open_onboarding(
        &mut self,
        step: FlowStep,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.onboarding.tutorial.is_some() {
            self.finish_tutorial(window, cx);
        }
        if matches!(self.sheet, Some(Sheet::Settings { .. })) {
            self.sheet = None;
        }
        self.onboarding.flow = Some(Flow {
            step,
            connecting: false,
            in_ai_settings: false,
            ai_rows: Vec::new(),
            message: None,
        });
        self.onboarding_go(step, window, cx);
    }

    fn onboarding_go(&mut self, step: FlowStep, window: &mut Window, cx: &mut Context<Self>) {
        let rows = if step == FlowStep::Ai {
            ais::rows(cx)
        } else {
            Vec::new()
        };
        if let Some(f) = self.onboarding.flow.as_mut() {
            f.step = step;
            f.message = None;
            f.ai_rows = rows;
        }
        window.focus(&self.onboarding.focus, cx);
        cx.notify();
    }

    fn onboarding_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(step) = self.onboarding.flow.as_ref().map(|f| f.step) else {
            return;
        };
        match step.next() {
            Some(n) => self.onboarding_go(n, window, cx),
            None => self.finish_onboarding(true, window, cx),
        }
    }

    fn onboarding_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.onboarding.flow.as_ref().and_then(|f| f.step.prev()) {
            self.onboarding_go(p, window, cx);
        }
    }

    /// Done or skipped: remember it, and optionally start the tour.
    pub(super) fn finish_onboarding(
        &mut self,
        then_tutorial: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.onboarding.flow = None;
        self.onboarding.onboarded_here = true;
        if let Some(dir) = data_dir(cx) {
            let _ = blyg_core::state::AppState::update(&dir, |s| s.onboarded = true);
        }
        if then_tutorial {
            self.start_tutorial(window, cx);
        } else {
            self.focus_after_sheet(window, cx);
        }
        cx.notify();
    }

    fn onboarding_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(f) = self.onboarding.flow.as_mut() {
            f.connecting = true;
        }
        self.open_connect(window, cx);
    }

    /// "Skip, just try it with sample data": this session runs on the
    /// FakeBackend (nothing is written to the config).
    fn onboarding_sample_data(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let fake = (self.onboarding.make_fake)();
        let as_backend: Arc<dyn blyg_core::Backend> = fake.clone();
        if !crate::connection::use_sample_data(as_backend.clone(), cx) {
            // No app global (headless tests): swap the window's own backend.
            let sink = self.onboarding_ui_sink(window, cx);
            as_backend.set_event_sink(sink);
            self.backend = as_backend;
        }
        self.fake = Some(fake);
        self.base_url = self.backend.base_url();
        self.reading = super::reading::State::new(&*self.backend, cx);
        self.current = None;
        self.requery(window, cx);
        self.show_toast(
            "Trying Blygger with sample data",
            Some("Nothing here reaches a blyg · connect one any time".into()),
            cx,
        );
        self.onboarding_next(window, cx);
    }

    fn onboarding_toggle_ai(&mut self, kind: ProviderKind, on: bool, cx: &mut Context<Self>) {
        let r = ais::set_enabled(kind, on, cx);
        let rows = ais::rows(cx);
        if let Some(f) = self.onboarding.flow.as_mut() {
            f.ai_rows = rows;
            f.message = r.err();
        }
        cx.notify();
    }

    fn onboarding_ai_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(f) = self.onboarding.flow.as_mut() {
            f.in_ai_settings = true;
        }
        self.ai_open_settings(window, cx);
    }

    /// "Buttons or keyboard?": `show-buttons = true|false`.
    fn onboarding_buttons(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        let v = if on { "true" } else { "false" };
        match crate::settings::write(&[(SHOW_BUTTONS, Change::Set(v.into()))], cx) {
            Ok(()) => {
                // Pick up whatever the prefs make of it (the toolbar), keeping
                // an unsaved dev theme override.
                let mut fresh = crate::settings::prefs(cx);
                self.persisted = fresh.clone();
                fresh.theme = self.prefs.theme;
                self.prefs = fresh;
                self.onboarding_next(window, cx);
            }
            Err(e) => {
                if let Some(f) = self.onboarding.flow.as_mut() {
                    f.message = Some(e);
                }
                cx.notify();
            }
        }
    }

    /// The flow waits behind the Connect sheet and Settings › AI; pick up
    /// when they close.
    fn onboarding_sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let connect_open = matches!(self.sheet, Some(Sheet::Connect { .. }));
        let ai_open = self.ai.has_overlay();
        let Some(f) = self.onboarding.flow.as_mut() else {
            return;
        };
        let mut refocus = false;
        if f.connecting && !connect_open {
            f.connecting = false;
            refocus = true;
            if crate::settings::blyg_url(cx).is_some() {
                f.step = FlowStep::Ai;
                f.ai_rows = ais::rows(cx);
            }
        }
        if let Some(f) = self.onboarding.flow.as_mut()
            && f.in_ai_settings
            && !ai_open
        {
            f.in_ai_settings = false;
            f.ai_rows = ais::rows(cx);
            refocus = true;
        }
        if refocus {
            cx.defer_in(window, |this, window, cx| {
                window.focus(&this.onboarding.focus, cx);
                cx.notify();
            });
        }
    }

    /// A sink that feeds core events to this window (used when the window
    /// swaps its own backend, i.e. without the app's `SwitchBackend`).
    fn onboarding_ui_sink(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Box<dyn Fn(blyg_core::CoreEvent) + Send + Sync> {
        let (tx, rx) = async_channel::unbounded::<blyg_core::CoreEvent>();
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            while let Ok(ev) = rx.recv().await {
                if this
                    .update_in(cx, |v, window, cx| v.on_core_event(ev, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
        Box::new(move |ev| {
            let _ = tx.try_send(ev);
        })
    }

    // ------------------------------------------------------------ drawing

    fn render_onboarding(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let f = self.onboarding.flow.as_ref()?;
        if f.connecting || f.in_ai_settings {
            return None;
        }
        let p = self.palette;
        let step = f.step;
        let kbd = |k: &'static str| {
            div()
                .px(px(6.))
                .py(px(1.))
                .min_w(px(20.))
                .flex()
                .justify_center()
                .rounded(px(5.))
                .border_1()
                .border_b_2()
                .border_color(p.line)
                .text_color(p.ink)
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(11.5))
                .child(k)
        };
        // A clickable choice with its key.
        let choice = |id: &'static str, key: &'static str, label: SharedString, primary: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(12.))
                .py(px(7.))
                .rounded(px(8.))
                .border_1()
                .cursor_pointer()
                .border_color(if primary { p.accent } else { p.line })
                .when(primary, |d| d.text_color(p.accent))
                .hover(|s| s.border_color(p.accent))
                .child(kbd(key))
                .child(label)
        };
        let para = |s: SharedString| {
            div()
                .text_color(p.muted)
                .line_height(relative(1.5))
                .child(s)
        };
        let host = crate::settings::blyg_url(cx)
            .map(|u| crate::vm::url_host(&u).unwrap_or(u))
            .filter(|_| crate::connection::mode(cx) != Some(crate::connection::Mode::Disconnected));

        let (title, body): (&str, AnyElement) = match step {
            FlowStep::Welcome => (
                "Welcome to Blygger",
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(para(
                        "A fast, keyboard-first studio for your blyg: search, write, publish \
                         and read from one window, with everything kept on this Mac too."
                            .into(),
                    ))
                    .child(
                        div()
                            .px(px(10.))
                            .py(px(7.))
                            .rounded(px(7.))
                            .bg(p.bar)
                            .text_size(px(11.5))
                            .text_color(p.muted)
                            .line_height(relative(1.45))
                            .child(
                                "Vibecoded: written with AI assistance, and provided as is, \
                                 with no warranty of any kind.",
                            ),
                    )
                    .child(div().mt(px(4.)).flex().gap(px(8.)).child(
                        choice("ob-start", "⏎", "Get started".into(), true).on_click(
                            cx.listener(|this, _, window, cx| this.onboarding_next(window, cx)),
                        ),
                    ))
                    .into_any_element(),
            ),
            FlowStep::Connect => (
                "Connect your blyg",
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(para(match &host {
                        Some(h) => format!(
                            "Connected to {h}. You can change it later from the Blygger menu."
                        )
                        .into(),
                        None => "Blygger writes to your own blyg. You'll need its address and its \
                                 owner token; the token goes in your macOS Keychain, the \
                                 address in your config file."
                            .into(),
                    }))
                    .child(div().mt(px(4.)).flex().flex_wrap().gap(px(8.)).map(|d| {
                        if host.is_some() {
                            d.child(
                                choice("ob-continue", "⏎", "Continue".into(), true).on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.onboarding_next(window, cx)
                                    }),
                                ),
                            )
                        } else {
                            d.child(choice("ob-connect", "⏎", "Connect…".into(), true).on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.onboarding_connect(window, cx)
                                }),
                            ))
                            .child(
                                choice(
                                    "ob-sample",
                                    "S",
                                    "Skip, just try it with sample data".into(),
                                    false,
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| this.onboarding_sample_data(window, cx),
                                )),
                            )
                        }
                    }))
                    .into_any_element(),
            ),
            FlowStep::Ai => {
                let rows = f.ai_rows.iter().map(|r| {
                    let kind = r.kind;
                    let (status, tone) = ais::status_label(r);
                    let free = ais::toggles_freely(kind);
                    let on = r.enabled;
                    let tone_color = match tone {
                        ais::Tone::Good => p.green,
                        ais::Tone::Neutral => p.muted,
                        ais::Tone::Bad => p.warn,
                    };
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .py(px(4.))
                        .border_b_1()
                        .border_color(p.line)
                        .child(
                            div()
                                .w(px(150.))
                                .flex_none()
                                .text_color(p.ink)
                                .child(kind.label()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(11.5))
                                .text_color(tone_color)
                                .child(status),
                        )
                        .child(if free {
                            div()
                                .id(SharedString::from(format!("ob-ai-{}", kind.config_name())))
                                .px(px(9.))
                                .py(px(2.))
                                .rounded_full()
                                .border_1()
                                .cursor_pointer()
                                .text_size(px(11.5))
                                .border_color(if on { p.accent } else { p.line })
                                .when(on, |d| d.text_color(p.accent))
                                .hover(|s| s.border_color(p.accent))
                                .child(if on { "On" } else { "Off" })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.onboarding_toggle_ai(kind, !on, cx)
                                }))
                                .into_any_element()
                        } else {
                            div()
                                .text_size(px(11.5))
                                .text_color(p.muted)
                                .child(if on { "On" } else { "in Settings" })
                                .into_any_element()
                        })
                });
                (
                    "AI is optional",
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(para(
                            "AI stays off until you switch a provider on: nothing is sent \
                             anywhere before that. Installed claude and codex CLIs are found \
                             on your PATH and use your own sign-in."
                                .into(),
                        ))
                        .child(div().flex().flex_col().children(rows))
                        .children(f.message.clone().map(|m| div().text_color(p.over).child(m)))
                        .child(
                            div()
                                .mt(px(4.))
                                .flex()
                                .flex_wrap()
                                .gap(px(8.))
                                .child(choice("ob-ai-next", "⏎", "Continue".into(), true).on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.onboarding_next(window, cx)
                                    }),
                                ))
                                .child(
                                    choice(
                                        "ob-ai-settings",
                                        "A",
                                        "Settings › AI accounts…".into(),
                                        false,
                                    )
                                    .on_click(cx.listener(
                                        |this, _, window, cx| {
                                            this.onboarding_ai_settings(window, cx)
                                        },
                                    )),
                                ),
                        )
                        .into_any_element(),
                )
            }
            FlowStep::Buttons => {
                let on = show_buttons(cx);
                (
                    "Buttons or keyboard?",
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .child(para(
                            "Blygger is keyboard-first. A quiet toolbar can show the main actions \
                             as buttons too, and every button's tooltip teaches its key. You can \
                             change this in Settings (⌘,)."
                                .into(),
                        ))
                        .children(f.message.clone().map(|m| div().text_color(p.over).child(m)))
                        .child(
                            div()
                                .mt(px(4.))
                                .flex()
                                .flex_wrap()
                                .gap(px(8.))
                                .child(
                                    choice("ob-buttons", "1", "Buttons and keys".into(), on)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.onboarding_buttons(true, window, cx)
                                        })),
                                )
                                .child(
                                    choice("ob-keyboard", "2", "Keyboard only".into(), !on)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.onboarding_buttons(false, window, cx)
                                        })),
                                ),
                        )
                        .into_any_element(),
                )
            }
            FlowStep::Tutorial => (
                "Take the tour?",
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(para(
                        "A short walk through the real window: you press the keys, it shows \
                         what they do. It runs on sample data, so your blyg isn't touched."
                            .into(),
                    ))
                    .child(
                        div()
                            .mt(px(4.))
                            .flex()
                            .flex_wrap()
                            .gap(px(8.))
                            .child(
                                choice("ob-tour", "⏎", "Start the tour".into(), true).on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.finish_onboarding(true, window, cx)
                                    }),
                                ),
                            )
                            .child(choice("ob-no-tour", "esc", "Skip".into(), false).on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.finish_onboarding(false, window, cx)
                                }),
                            )),
                    )
                    .into_any_element(),
            ),
        };

        let dots = FlowStep::ALL.iter().map(|s| {
            div()
                .size(px(6.))
                .rounded_full()
                .bg(if *s == step { p.accent } else { p.line })
        });
        let footer =
            div()
                .mt(px(16.))
                .pt(px(10.))
                .border_t_1()
                .border_color(p.line)
                .flex()
                .items_center()
                .gap(px(14.))
                .text_size(px(11.5))
                .text_color(p.muted)
                .child(div().flex().gap(px(5.)).children(dots))
                .child(div().flex_1())
                .when(step.prev().is_some(), |d| {
                    d.child(
                        div()
                            .id("ob-back")
                            .cursor_pointer()
                            .hover(|s| s.text_color(p.ink))
                            .child("‹ Back")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.onboarding_back(window, cx)),
                            ),
                    )
                })
                .child(
                    div()
                        .id("ob-skip")
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .cursor_pointer()
                        .hover(|s| s.text_color(p.ink))
                        .child(kbd("esc"))
                        .child(if step == FlowStep::Tutorial {
                            "skip"
                        } else {
                            "skip setup"
                        })
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.finish_onboarding(false, window, cx)
                        })),
                );

        let card = div()
            .id("onboarding")
            .occlude()
            .track_focus(&self.onboarding.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if this.onboarding_key(ev, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .w(px(540.))
            .max_w(relative(0.92))
            .bg(p.bg)
            .border_1()
            .border_color(p.line)
            .rounded(px(12.))
            .shadow(vec![BoxShadow {
                color: p.shadow,
                offset: point(px(0.), px(18.)),
                blur_radius: px(48.),
                spread_radius: px(-10.),
                inset: false,
            }])
            .px(px(22.))
            .py(px(18.))
            .font_family(SharedString::from(self.prefs.ui().family))
            .text_size(px(13.))
            .child(
                div()
                    .text_size(px(10.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.muted)
                    .child(format!(
                        "SETUP · {} OF {}",
                        step.index() + 1,
                        FlowStep::ALL.len()
                    )),
            )
            .child(
                div()
                    .mt(px(4.))
                    .mb(px(10.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(17.))
                    .child(title),
            )
            .child(body)
            .child(footer);

        Some(
            div()
                .id("onboarding-scrim")
                .absolute()
                .inset_0()
                .occlude()
                .bg(p.ink.opacity(if p.dark { 0.35 } else { 0.12 }))
                .flex()
                .items_center()
                .justify_center()
                .child(card)
                .into_any_element(),
        )
    }

    /// The card's keys: esc skips, ⏎ goes on, and each step's letters.
    fn onboarding_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(step) = self.onboarding.flow.as_ref().map(|f| f.step) else {
            return false;
        };
        let m = &ev.keystroke.modifiers;
        if m.platform || m.control || m.alt {
            return false;
        }
        let connected = crate::settings::blyg_url(cx).is_some()
            && crate::connection::mode(cx) != Some(crate::connection::Mode::Disconnected);
        match (step, ev.keystroke.key.as_str()) {
            (_, "escape") => self.finish_onboarding(false, window, cx),
            (FlowStep::Connect, "enter") if !connected => self.onboarding_connect(window, cx),
            (FlowStep::Connect, "s") if !connected => self.onboarding_sample_data(window, cx),
            (FlowStep::Ai, "a") => self.onboarding_ai_settings(window, cx),
            (FlowStep::Buttons, "1") => self.onboarding_buttons(true, window, cx),
            (FlowStep::Buttons, "2") => self.onboarding_buttons(false, window, cx),
            (FlowStep::Buttons, "enter") => {
                let on = show_buttons(cx);
                self.onboarding_buttons(on, window, cx)
            }
            (FlowStep::Tutorial, "enter") => self.finish_onboarding(true, window, cx),
            (_, "enter") => self.onboarding_next(window, cx),
            (_, "left") => self.onboarding_back(window, cx),
            _ => return false,
        }
        true
    }

    // ------------------------------------------------------------ settings

    /// Settings › Help: replay the tour, show onboarding again, and the
    /// tutorial-on-launch checkbox.
    pub(super) fn render_help_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        let chip = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px(px(10.))
                .py(px(3.))
                .rounded_full()
                .border_1()
                .cursor_pointer()
                .border_color(p.line)
                .hover(|s| s.border_color(p.accent))
                .child(label)
        };
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
                    .child("HELP"),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(6.))
                            .child(chip("help-replay", "Replay tutorial").on_click(
                                cx.listener(|this, _, window, cx| this.help_replay(window, cx)),
                            ))
                            .child(chip("help-onboarding", "Show onboarding again").on_click(
                                cx.listener(|this, _, window, cx| this.help_onboarding(window, cx)),
                            )),
                    )
                    .child(self.render_tutorial_checkbox("help-on-launch", cx)),
            )
            .into_any_element()
    }

    /// Settings › Help › Replay tutorial.
    pub(super) fn help_replay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sheet = None;
        if self.onboarding.tutorial.is_some() {
            self.tutorial_enter(0, window, cx);
        } else {
            self.start_tutorial(window, cx);
        }
    }

    /// Settings › Help › Show onboarding again.
    pub(super) fn help_onboarding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sheet = None;
        self.open_onboarding(FlowStep::Welcome, window, cx);
    }

    /// "Show this tutorial every time I open Blygger" (`tutorial-on-launch`).
    pub(super) fn render_tutorial_checkbox(
        &self,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette;
        let on = tutorial_on_launch(cx);
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(7.))
            .cursor_pointer()
            .text_size(px(11.5))
            .text_color(p.muted)
            .hover(|s| s.text_color(p.ink))
            .child(
                div()
                    .size(px(13.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(if on { p.accent } else { p.muted })
                    .when(on, |d| d.bg(p.accent).text_color(p.bg))
                    .text_size(px(10.))
                    .child(if on { "✓" } else { "" }),
            )
            .child("Show this tutorial every time I open Blygger")
            .on_click(cx.listener(move |this, _, _, cx| this.set_tutorial_on_launch(!on, cx)))
            .into_any_element()
    }

    pub(super) fn set_tutorial_on_launch(&mut self, on: bool, cx: &mut Context<Self>) {
        let v = if on { "true" } else { "false" };
        if let Err(e) = crate::settings::write(&[(TUTORIAL_ON_LAUNCH, Change::Set(v.into()))], cx) {
            self.show_toast(e, None, cx);
        }
        cx.notify();
    }

    // ------------------------------------------------------------ demos

    /// `BLYGGER_DEMO=ob-…` / `tut-…` (screenshots): the same calls a click
    /// or a key makes.
    pub(super) fn onboarding_demo(
        &mut self,
        scenario: &str,
        n: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match (scenario, n) {
            ("ob-welcome", 0) => self.open_onboarding(FlowStep::Welcome, window, cx),
            ("ob-connect", 0) => self.open_onboarding(FlowStep::Connect, window, cx),
            ("ob-ai", 0) => self.open_onboarding(FlowStep::Ai, window, cx),
            ("ob-buttons", 0) => self.open_onboarding(FlowStep::Buttons, window, cx),
            ("ob-tour", 0) => self.open_onboarding(FlowStep::Tutorial, window, cx),
            (s, 0) if s.starts_with("tut-") => {
                self.start_tutorial(window, cx);
                let id = s.trim_start_matches("tut-");
                if let Some(i) = steps::STEPS.iter().position(|st| st.id == id) {
                    self.tutorial_enter(i, window, cx);
                }
            }
            _ => {}
        }
        if n == 2 {
            tutorial::snapshot_later(window, cx);
        }
    }
}
