//! Quick capture: a global hotkey (default ⌃⌥B) shows a small floating panel
//! over any app. esc, ⌘S or clicking away keeps the text as a scratch note
//! that stays on this Mac (or a draft, with `capture-default = draft`); ⌘D
//! saves it as a draft on the blyg; ⌘⏎ publishes it (docs/SPEC.md § Scratch
//! notes). A long note publishes as a thread (`blyg_core::promotion_kind`).
//!
//! The hotkey uses the `global-hotkey` crate (Carbon `RegisterEventHotKey`
//! under the hood), whose events fire on the main run loop that GPUI's
//! NSApplication already drives. We forward them over a channel into a GPUI
//! foreground task. The panel is a `WindowKind::PopUp` (a non-activating
//! NSPanel at pop-up level that joins all Spaces), so it takes keys without
//! pulling the main window forward.

use std::sync::Arc;
use std::time::Duration;

use blyg_core::config::CaptureDefault;
use blyg_core::{Backend, Kind, Promote};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui_kit::base::input::{Enter, Escape, InputEditorStyle, InputEvent, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::scratch::{KeepCapture, MakeDraft};
use crate::app::{MainView, Publish};
use crate::prefs::{self, Prefs};
use crate::theme::Palette;
use crate::vm;

struct CaptureGlobal {
    manager: Option<GlobalHotKeyManager>,
    hotkey: Option<HotKey>,
    window: Option<WindowHandle<CaptureView>>,
    backend: Arc<dyn Backend>,
    prefs: Prefs,
    main: Option<WindowHandle<MainView>>,
}

impl Global for CaptureGlobal {}

pub fn init(backend: Arc<dyn Backend>, prefs: &Prefs, cx: &mut App) {
    let manager = match GlobalHotKeyManager::new() {
        Ok(m) => Some(m),
        Err(e) => {
            eprintln!("blygger: global hotkeys unavailable: {e}");
            None
        }
    };
    let hotkey =
        prefs::parse_hotkey(&prefs.hotkey).or_else(|| prefs::parse_hotkey(prefs::DEFAULT_HOTKEY));
    if let (Some(m), Some(h)) = (&manager, hotkey)
        && let Err(e) = m.register(h)
    {
        eprintln!("blygger: couldn't register {}: {e}", prefs.hotkey);
    }
    cx.set_global(CaptureGlobal {
        manager,
        hotkey,
        window: None,
        backend,
        prefs: prefs.clone(),
        main: None,
    });

    let (tx, rx) = async_channel::unbounded::<GlobalHotKeyEvent>();
    GlobalHotKeyEvent::set_event_handler(Some(move |e| {
        let _ = tx.try_send(e);
    }));
    cx.spawn(async move |cx| {
        while let Ok(ev) = rx.recv().await {
            if ev.state != HotKeyState::Pressed {
                continue;
            }
            cx.update(|cx| {
                let ours = cx.global::<CaptureGlobal>().hotkey.map(|h| h.id()) == Some(ev.id);
                if ours {
                    toggle(cx);
                }
            });
        }
    })
    .detach();
}

pub fn set_main(handle: WindowHandle<MainView>, cx: &mut App) {
    cx.global_mut::<CaptureGlobal>().main = Some(handle);
}

pub fn main_window(cx: &App) -> Option<WindowHandle<MainView>> {
    cx.global::<CaptureGlobal>().main
}

pub fn prefs_changed(prefs: &Prefs, cx: &mut App) {
    // (No capture global in headless tests.)
    if cx.has_global::<CaptureGlobal>() {
        cx.global_mut::<CaptureGlobal>().prefs = prefs.clone();
    }
}

/// Swap the global hotkey. On failure the old one stays registered.
pub fn set_hotkey(text: &str, cx: &mut App) -> Result<(), String> {
    let new = prefs::parse_hotkey(text)
        .ok_or_else(|| format!("“{}” isn't a key combination I understand", text.trim()))?;
    if !cx.has_global::<CaptureGlobal>() {
        return Err("Global hotkeys are unavailable here".into());
    }
    let g = cx.global_mut::<CaptureGlobal>();
    let Some(manager) = &g.manager else {
        return Err("Global hotkeys are unavailable on this system".into());
    };
    if g.hotkey == Some(new) {
        return Ok(());
    }
    if let Some(old) = g.hotkey {
        let _ = manager.unregister(old);
    }
    match manager.register(new) {
        Ok(()) => {
            g.hotkey = Some(new);
            Ok(())
        }
        Err(e) => {
            if let Some(old) = g.hotkey {
                let _ = manager.register(old);
            }
            Err(format!("Couldn't use that combination: {e}"))
        }
    }
}

pub fn toggle(cx: &mut App) {
    if let Some(handle) = cx.global::<CaptureGlobal>().window {
        // Already open: bring it back to the front (it may be behind a Space switch).
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
        cx.global_mut::<CaptureGlobal>().window = None;
    }
    open(cx);
}

const W: f32 = 520.;
const H: f32 = 170.;

fn open(cx: &mut App) {
    let g = cx.global::<CaptureGlobal>();
    let backend = g.backend.clone();
    let prefs = g.prefs.clone();
    crate::fonts::ensure(prefs.writing().bundled, cx);
    crate::fonts::ensure(prefs.ui().bundled, cx);
    let bounds = cx
        .primary_display()
        .map(|d| {
            let b = d.bounds();
            Bounds {
                origin: point(
                    b.origin.x + (b.size.width - px(W)) / 2.,
                    b.origin.y + b.size.height * 0.22,
                ),
                size: size(px(W), px(H)),
            }
        })
        .unwrap_or_else(|| Bounds::centered(None, size(px(W), px(H)), cx));
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        focus: !crate::no_activate(),
        show: true,
        kind: WindowKind::PopUp,
        is_movable: true,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    };
    match cx.open_window(opts, |window, cx| {
        cx.new(|cx| CaptureView::new(backend, prefs, window, cx))
    }) {
        Ok(handle) => {
            if crate::no_activate() {
                let _ = handle.update(cx, |_, window, _| crate::order_front_regardless(window));
            } else {
                let _ = handle.update(cx, |_, window, _| window.activate_window());
            }
            cx.global_mut::<CaptureGlobal>().window = Some(handle);
        }
        Err(e) => eprintln!("blygger: couldn't open quick capture: {e}"),
    }
}

fn refresh_main(cx: &mut App) {
    if let Some(h) = main_window(cx) {
        let _ = h.update(cx, |view, window, cx| view.refresh_from_backend(window, cx));
    }
}

pub struct CaptureView {
    backend: Arc<dyn Backend>,
    editor: Entity<TextareaState>,
    prefs: Prefs,
    done: Option<SharedString>,
    note: Option<SharedString>,
    /// The text has been kept (or is being published): don't keep it again
    /// when the panel loses focus on its way out.
    finished: bool,
    /// Seen the panel active, so a later deactivation is a click away.
    was_active: bool,
    _subs: Vec<Subscription>,
}

// --- buttons ---

/// One button of the panel's Scratch · Draft · Publish row (`show-buttons`).
#[derive(Debug, Clone)]
pub struct CaptureButton {
    pub action: &'static str,
    pub label: &'static str,
    pub icon: &'static str,
    /// The key, shown after the label (the row replaces the key hints).
    pub key: String,
    pub tooltip: String,
    pub reason: Option<&'static str>,
    new_action: fn() -> Box<dyn Action>,
}

/// The row, from `keymap::CAPTURE_ROW` and the keymap table. With
/// `capture-default = draft` there's no Scratch button: esc keeps a draft.
pub fn capture_buttons(empty: bool, capture_default: CaptureDefault) -> Vec<CaptureButton> {
    crate::keymap::CAPTURE_ROW
        .iter()
        .filter(|(a, _)| !(*a == "KeepCapture" && capture_default == CaptureDefault::Draft))
        .filter_map(|&(action, label)| {
            let k = crate::keymap::button_row(action)?;
            let mut tooltip = crate::keymap::tooltip(label, &k, "");
            if action == "KeepCapture" {
                tooltip.push_str(" · esc");
            }
            let reason = empty.then_some(match action {
                "KeepCapture" => "Nothing to keep yet",
                "MakeDraft" => "Nothing to save as a draft yet",
                _ => "Nothing to publish yet",
            });
            Some(CaptureButton {
                action,
                label,
                icon: k.icon.unwrap_or("send"),
                key: crate::keymap::shortcut(&k, ""),
                tooltip,
                reason,
                new_action: k.new_action?,
            })
        })
        .collect()
}
// --- end buttons ---

/// How quick capture keeps its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Keep {
    Scratch,
    Draft,
}

impl Keep {
    fn default_for(c: CaptureDefault) -> Keep {
        match c {
            CaptureDefault::Scratch => Keep::Scratch,
            CaptureDefault::Draft => Keep::Draft,
        }
    }
}

impl CaptureView {
    fn new(
        backend: Arc<dyn Backend>,
        prefs: Prefs,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = cx.new(|cx| {
            TextareaState::new(window, cx)
                .soft_wrap(true)
                .placeholder("What's on your mind…")
        });
        editor.update(cx, |s, cx| s.focus(window, cx));
        let subs = vec![
            cx.subscribe(&editor, |this: &mut Self, _, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.note = None;
                    cx.notify();
                }
            }),
            cx.observe_window_appearance(window, |_, _, cx| cx.notify()),
            // Clicking away keeps the note, like esc.
            cx.observe_window_activation(window, |this: &mut Self, window, cx| {
                if window.is_window_active() {
                    this.was_active = true;
                } else if this.was_active {
                    let how = Keep::default_for(this.prefs.capture_default);
                    this.keep(how, window, cx);
                }
            }),
        ];
        Self {
            backend,
            editor,
            prefs,
            done: None,
            note: None,
            finished: false,
            was_active: window.is_window_active(),
            _subs: subs,
        }
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.global_mut::<CaptureGlobal>().window = None;
        window.remove_window();
    }

    /// esc / ⌘S / click-away (`capture-default`) and ⌘D (always a draft).
    fn keep(&mut self, how: Keep, window: &mut Window, cx: &mut Context<Self>) {
        if self.finished || self.done.is_some() {
            return;
        }
        self.finished = true;
        let text = self.editor.read(cx).value().trim_end().to_string();
        if !text.trim().is_empty() {
            let saved = match how {
                Keep::Scratch => self.backend.create_scratch(Kind::Fragment, &text),
                Keep::Draft => self.backend.create_draft(Kind::Fragment, &text),
            };
            if let Err(e) = saved {
                // Stay open so the text isn't lost.
                self.finished = false;
                self.note = Some(format!("Couldn't save: {e}").into());
                cx.notify();
                return;
            }
            refresh_main(cx);
        }
        self.close(window, cx);
    }

    fn publish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.done.is_some() || self.finished {
            return;
        }
        let text = self.editor.read(cx).value().trim_end().to_string();
        if text.trim().is_empty() {
            return;
        }
        // Kept locally first, so a failed publish still leaves the note
        // (promoted to a queued draft).
        let id = match self.backend.create_scratch(Kind::Fragment, &text) {
            Ok(id) => id,
            Err(e) => {
                self.note = Some(format!("Couldn't save: {e}").into());
                cx.notify();
                return;
            }
        };
        self.finished = true;
        self.done = Some("Publishing…".into());
        cx.notify();
        let backend = self.backend.clone();
        let task = cx
            .background_spawn(async move { backend.promote(&id, Promote::Publish { note: None }) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let msg: SharedString = match &result {
                Ok(p) if p.kind == Kind::Thread => "Published as a thread ✓".into(),
                Ok(_) => "Published ✓".into(),
                Err(e) => format!("Kept as a draft · {e}").into(),
            };
            let _ = this.update(cx, |v, cx| {
                v.done = Some(msg);
                cx.notify();
                refresh_main(cx);
            });
            let wait = if result.is_ok() { 900 } else { 1800 };
            cx.background_executor()
                .timer(Duration::from_millis(wait))
                .await;
            let _ = this.update_in(cx, |v, window, cx| v.close(window, cx));
        })
        .detach();
    }
}

impl Render for CaptureView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::resolve(self.prefs.theme, window.appearance());
        let body_font: SharedString = self.prefs.writing().family.into();
        let ui_font: SharedString = self.prefs.ui().family.into();
        self.editor.update(cx, |s, _| {
            s.set_editor_style(InputEditorStyle {
                foreground: p.ink,
                muted_foreground: p.muted,
                background: p.bg,
                border: p.line,
                selection: p.text_selection,
                caret: p.accent,
                ..Default::default()
            });
            s.set_editor_paddings(Edges {
                top: px(18.),
                bottom: px(6.),
                left: px(20.),
                right: px(20.),
            });
        });
        let chars = self.editor.read(cx).text().len_chars();
        let (counter, level) = vm::capture_counter(chars);
        let esc_label = match self.prefs.capture_default {
            CaptureDefault::Scratch => "scratch",
            CaptureDefault::Draft => "keep draft",
        };
        let kbd = |k: &'static str| {
            div()
                .px(px(5.))
                .rounded(px(4.))
                .border_1()
                .border_b_2()
                .border_color(p.line)
                .text_color(p.ink)
                .text_size(px(10.5))
                .child(k)
        };

        // --- buttons --- the Scratch · Draft · Publish row replaces the key hints.
        let buttons = self.prefs.show_buttons.then(|| {
            let empty = self.editor.read(cx).value().trim().is_empty() || self.done.is_some();
            let row = capture_buttons(empty, self.prefs.capture_default)
                .into_iter()
                .map(|b| {
                    let make = b.new_action;
                    crate::app::toolbar::button_element(
                        format!("cap-{}", b.action).into(),
                        b.icon,
                        Some(b.label),
                        Some(b.key),
                        b.tooltip,
                        b.reason.map(str::to_string),
                        false,
                        &p,
                    )
                    .when(b.reason.is_none(), |d| {
                        d.on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            window.dispatch_action(make(), cx);
                        })
                    })
                });
            div().flex().items_center().gap(px(2.)).children(row)
        });
        let footer = div()
            .flex_none()
            .flex()
            .justify_between()
            .items_center()
            .px(px(16.))
            .py(px(8.))
            .border_t_1()
            .border_color(p.line)
            .font_family(ui_font.clone())
            .text_size(px(11.5))
            .text_color(p.muted)
            .child(
                div()
                    .when(level == vm::Level::Warn, |d| d.text_color(p.warn))
                    .when(level == vm::Level::Over, |d| {
                        d.text_color(p.over).font_weight(FontWeight::SEMIBOLD)
                    })
                    .child(self.note.clone().unwrap_or_else(|| counter.into())),
            )
            .map(|d| match buttons {
                Some(row) => d.child(row), // --- buttons ---
                None => d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .child(kbd(crate::keymap::hint("⌘⏎")))
                        .child("publish")
                        .child(div().w(px(8.)))
                        .child(kbd(crate::keymap::hint("⌘D")))
                        .child("draft")
                        .child(div().w(px(8.)))
                        .child(kbd("esc"))
                        .child(esc_label),
                ),
            });

        let card = div()
            .id("capture")
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .rounded(px(14.))
            .overflow_hidden()
            .bg(p.bg)
            .border_1()
            .border_color(p.line)
            .text_color(p.ink)
            .capture_action(cx.listener(|this, a: &Enter, window, cx| {
                if a.secondary {
                    cx.stop_propagation();
                    this.publish(window, cx);
                }
            }))
            .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                cx.stop_propagation();
                let how = Keep::default_for(this.prefs.capture_default);
                this.keep(how, window, cx);
            }))
            .on_action(cx.listener(|this, _: &KeepCapture, window, cx| {
                let how = Keep::default_for(this.prefs.capture_default);
                this.keep(how, window, cx);
            }))
            .on_action(cx.listener(|this, _: &MakeDraft, window, cx| {
                this.keep(Keep::Draft, window, cx);
            }))
            // --- buttons --- the Publish button (⌘⏎ itself arrives as Enter above).
            .on_action(cx.listener(|this, _: &Publish, window, cx| {
                this.publish(window, cx);
            }))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .font_family(body_font.clone())
                    .text_size(px(self.prefs.font_size))
                    .line_height(relative(1.5))
                    .child(Textarea::new(&self.editor)),
            )
            .child(footer)
            .when_some(self.done.clone(), |d, msg| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(p.bg)
                        .font_family(body_font.clone())
                        .italic()
                        .text_size(px(22.))
                        .child(msg)
                        .with_animation(
                            "done",
                            Animation::new(Duration::from_millis(200)),
                            |d, t| d.opacity(t),
                        ),
                )
            });

        div().size_full().child(card).with_animation(
            "qc-in",
            Animation::new(Duration::from_millis(180)).with_easing(ease_out_quint()),
            |d, t| d.opacity(t).mt(px(-12.0 * (1.0 - t))),
        )
    }
}

/// Demo hook: put text in an open capture panel.
pub fn demo_fill(text: &str, cx: &mut App) {
    if let Some(h) = cx.global::<CaptureGlobal>().window {
        let text = text.to_string();
        let _ = h.update(cx, |v, window, cx| {
            v.editor.update(cx, |s, cx| s.set_value(text, window, cx));
            cx.notify();
        });
    }
}

/// --- buttons --- Demo hook: save the open panel's frame to a PNG (only in
/// a `blygger_snap` build; see reading/demo.rs), then quit.
pub fn demo_snapshot(path: &str, cx: &mut App) {
    let Some(h) = cx.global::<CaptureGlobal>().window else {
        return;
    };
    let path = path.to_string();
    let _ = h.update(cx, |_, window, cx| {
        cx.spawn_in(window, async move |_, cx| {
            for _ in 0..3 {
                let _ = cx.update(|window, cx| window.draw(cx).clear(cx));
                cx.background_executor()
                    .timer(Duration::from_millis(300))
                    .await;
            }
            let _ = cx.update(|window, cx| {
                window.draw(cx).clear(cx);
                crate::app::reading::demo::snap::save_frame(window, &path);
                cx.quit();
            });
        })
        .detach();
    });
}

#[cfg(test)]
mod tests {
    //! The panel's keys, headless, against a zero-latency FakeBackend.
    use std::sync::Arc;

    use blyg_core::{Backend, ConfigStore, Kind, Status};
    use gpui_kit::{TestAppContext, VisualTestContext};

    use super::{CaptureGlobal, CaptureView};
    use crate::fake::{FakeBackend, Timing};
    use crate::prefs::Prefs;

    fn open_panel<'a>(
        cx: &'a mut TestAppContext,
        config: &str,
    ) -> (Arc<FakeBackend>, &'a mut VisualTestContext) {
        let prefs = Prefs::from_config(ConfigStore::in_memory(config).config());
        let fake = Arc::new(FakeBackend::with_timing(Timing::instant()).without_media_cache());
        let backend: Arc<dyn Backend> = fake.clone();
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::app::bind_keys(cx);
            cx.set_global(CaptureGlobal {
                manager: None,
                hotkey: None,
                window: None,
                backend: backend.clone(),
                prefs: prefs.clone(),
                main: None,
            });
        });
        let (_view, cx) =
            cx.add_window_view(move |window, cx| CaptureView::new(backend, prefs, window, cx));
        cx.run_until_parked();
        (fake, cx)
    }

    /// The newest item, if the panel kept one (the fake is seeded with six).
    fn kept(fake: &FakeBackend) -> Option<blyg_core::Item> {
        let items = fake.items();
        (items.len() == 7).then(|| items[0].clone())
    }

    #[gpui_kit::test]
    fn esc_keeps_a_scratch_note(cx: &mut TestAppContext) {
        let (fake, cx) = open_panel(cx, "");
        cx.simulate_input("buy more string");
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        let it = kept(&fake).expect("kept");
        assert_eq!(it.content_md, "buy more string");
        assert_eq!(it.status, Status::Scratch);
        assert!(!it.pending_sync);
    }

    #[gpui_kit::test]
    fn cmd_s_keeps_a_scratch_note(cx: &mut TestAppContext) {
        let (fake, cx) = open_panel(cx, "");
        cx.simulate_input("the owl again, 3am");
        cx.simulate_keystrokes(&crate::keymap::keys("cmd-s"));
        cx.run_until_parked();
        assert_eq!(kept(&fake).expect("kept").status, Status::Scratch);
    }

    #[gpui_kit::test]
    fn clicking_away_keeps_a_scratch_note(cx: &mut TestAppContext) {
        let (fake, cx) = open_panel(cx, "");
        cx.simulate_input("thought on the stairs");
        cx.run_until_parked();
        cx.update(|w, _| w.activate_window());
        cx.run_until_parked();
        assert!(cx.update(|w, _| w.is_window_active()));
        assert!(kept(&fake).is_none(), "activating keeps nothing");
        cx.deactivate_window();
        cx.run_until_parked();
        let it = kept(&fake).expect("kept");
        assert_eq!(it.status, Status::Scratch);
        assert_eq!(fake.items().len(), 7, "kept once");
    }

    #[gpui_kit::test]
    fn capture_default_draft_keeps_a_draft(cx: &mut TestAppContext) {
        let (fake, cx) = open_panel(cx, "capture-default = draft\n");
        cx.simulate_input("the owl again, 3am");
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert_eq!(kept(&fake).expect("kept").status, Status::Draft);
    }

    #[gpui_kit::test]
    fn cmd_d_saves_a_draft(cx: &mut TestAppContext) {
        let (fake, cx) = open_panel(cx, "");
        cx.simulate_input("draft me");
        cx.simulate_keystrokes(&crate::keymap::keys("cmd-d"));
        cx.run_until_parked();
        assert_eq!(kept(&fake).expect("kept").status, Status::Draft);
    }

    #[gpui_kit::test]
    fn cmd_enter_publishes(cx: &mut TestAppContext) {
        let (fake, cx) = open_panel(cx, "");
        cx.simulate_input("published from the panel");
        cx.simulate_keystrokes(&crate::keymap::keys("cmd-enter"));
        cx.run_until_parked();
        let it = kept(&fake).expect("kept");
        assert_eq!((it.status, it.version), (Status::Public, 1));
        assert_eq!(it.kind, Kind::Fragment);
    }

    #[gpui_kit::test]
    fn empty_text_keeps_nothing(cx: &mut TestAppContext) {
        let (fake, cx) = open_panel(cx, "");
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(kept(&fake).is_none());
    }

    // --- buttons ---

    fn click(cx: &mut VisualTestContext, selector: &'static str) {
        let b = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} isn't on screen"));
        cx.simulate_click(b.center(), gpui_kit::Modifiers::none());
        cx.run_until_parked();
    }

    #[test]
    fn the_row_is_scratch_draft_publish_from_the_keymap() {
        use super::capture_buttons;
        use blyg_core::config::CaptureDefault;
        let row = capture_buttons(false, CaptureDefault::Scratch);
        let got: Vec<(&str, &str, &str)> = row
            .iter()
            .map(|b| (b.label, b.key.as_str(), b.tooltip.as_str()))
            .collect();
        // The keys as this platform shows them (⌘S on macOS, Ctrl+S on Windows).
        let key = |k: &str| crate::keymap::glyphs(&crate::keymap::keys(k));
        let (s, d, p) = (key("cmd-s"), key("cmd-d"), key("cmd-enter"));
        let (ts, td, tp) = (
            format!("Scratch  {s} · esc"),
            format!("Draft  {d}"),
            format!("Publish  {p}"),
        );
        assert_eq!(
            got,
            [
                ("Scratch", s.as_str(), ts.as_str()),
                ("Draft", d.as_str(), td.as_str()),
                ("Publish", p.as_str(), tp.as_str()),
            ]
        );
        assert!(row.iter().all(|b| b.reason.is_none()));
        let empty = capture_buttons(true, CaptureDefault::Scratch);
        assert_eq!(empty[2].reason, Some("Nothing to publish yet"));
        // capture-default = draft: esc keeps a draft, so no Scratch button.
        let row = capture_buttons(false, CaptureDefault::Draft);
        let labels: Vec<&str> = row.iter().map(|b| b.label).collect();
        assert_eq!(labels, ["Draft", "Publish"]);
    }

    #[gpui_kit::test]
    fn clicking_publish_in_the_row_publishes(cx: &mut TestAppContext) {
        let (fake, cx) = open_panel(cx, "");
        cx.simulate_input("published by a click");
        cx.run_until_parked();
        click(cx, "cap-Publish");
        let it = kept(&fake).expect("kept");
        assert_eq!((it.status, it.version), (Status::Public, 1));
    }

    #[gpui_kit::test]
    fn clicking_draft_in_the_row_saves_a_draft(cx: &mut TestAppContext) {
        let (fake, cx) = open_panel(cx, "");
        cx.simulate_input("a draft by a click");
        cx.run_until_parked();
        click(cx, "cap-MakeDraft");
        assert_eq!(kept(&fake).expect("kept").status, Status::Draft);
    }

    #[gpui_kit::test]
    fn clicking_scratch_in_the_row_keeps_a_scratch_note(cx: &mut TestAppContext) {
        let (fake, cx) = open_panel(cx, "");
        cx.simulate_input("a scratch by a click");
        cx.run_until_parked();
        click(cx, "cap-KeepCapture");
        assert_eq!(kept(&fake).expect("kept").status, Status::Scratch);
    }

    #[gpui_kit::test]
    fn empty_panel_row_is_disabled(cx: &mut TestAppContext) {
        let (fake, cx) = open_panel(cx, "");
        click(cx, "cap-Publish");
        assert!(kept(&fake).is_none(), "nothing to publish");
    }

    #[gpui_kit::test]
    fn show_buttons_false_keeps_the_key_hints(cx: &mut TestAppContext) {
        let (_fake, cx) = open_panel(cx, "show-buttons = false\n");
        assert!(cx.debug_bounds("cap-Publish").is_none());
        assert!(cx.debug_bounds("cap-KeepCapture").is_none());
    }
}
