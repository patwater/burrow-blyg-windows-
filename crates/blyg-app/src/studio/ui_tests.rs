//! Headless GPUI tests of the full editor: mode keys, rendering into the
//! preview surface (a recording stub, so no WKWebView is needed), the
//! typing debounce, source ↔ preview jumps and the fallback.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use blyg_core::config::MemoryTokenStore;
use blyg_core::{Backend, ConfigStore, Kind};
use gpui_kit::{Bounds, Entity, Pixels, TestAppContext, VisualTestContext};

use super::webview::{FactoryGlobal, PreviewSurface, SurfaceEvent};
use super::{MainView, ViewMode};
use crate::app::{Mode, Sheet};
use crate::fake::{FakeBackend, SAMPLE_QUOTE_ID, Timing};
use crate::prefs::Prefs;

#[derive(Debug, Clone, PartialEq)]
enum Call {
    Frame(Bounds<Pixels>),
    Visible(bool),
    Load(String),
    Eval(String),
    FocusParent,
    Reclaim,
    Dark(bool),
}

#[derive(Default)]
struct Log(Vec<Call>);

impl Log {
    fn loads(&self) -> Vec<&str> {
        self.0
            .iter()
            .filter_map(|c| match c {
                Call::Load(h) => Some(h.as_str()),
                _ => None,
            })
            .collect()
    }
    fn evals(&self) -> Vec<&str> {
        self.0
            .iter()
            .filter_map(|c| match c {
                Call::Eval(h) => Some(h.as_str()),
                _ => None,
            })
            .collect()
    }
    fn visible(&self) -> Option<bool> {
        self.0.iter().rev().find_map(|c| match c {
            Call::Visible(v) => Some(*v),
            _ => None,
        })
    }
}

struct Stub(Rc<RefCell<Log>>);

impl PreviewSurface for Stub {
    fn set_frame(&mut self, b: Bounds<Pixels>) {
        self.0.borrow_mut().0.push(Call::Frame(b));
    }
    fn set_visible(&mut self, v: bool) {
        self.0.borrow_mut().0.push(Call::Visible(v));
    }
    fn load(&mut self, html: &str) {
        self.0.borrow_mut().0.push(Call::Load(html.to_string()));
    }
    fn eval(&mut self, js: &str) {
        self.0.borrow_mut().0.push(Call::Eval(js.to_string()));
    }
    fn focus_parent(&mut self) {
        self.0.borrow_mut().0.push(Call::FocusParent);
    }
    fn set_dark(&mut self, dark: bool) {
        self.0.borrow_mut().0.push(Call::Dark(dark));
    }
    fn reclaim_keyboard(&mut self) {
        self.0.borrow_mut().0.push(Call::Reclaim);
    }
}

type Setup<'a> = (
    Entity<MainView>,
    Arc<FakeBackend>,
    Rc<RefCell<Log>>,
    &'a mut VisualTestContext,
);

fn setup(cx: &mut TestAppContext, webview_works: bool) -> Setup<'_> {
    let config = "blyg-url = https://blyg.example.com\n".to_string();
    let prefs = Prefs::from_config(ConfigStore::in_memory(&config).config());
    let log = Rc::new(RefCell::new(Log::default()));
    let l2 = log.clone();
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::app::bind_keys(cx);
        crate::settings::init(
            ConfigStore::in_memory(&config),
            Arc::new(MemoryTokenStore::default()),
            None,
            cx,
        );
        cx.set_global(FactoryGlobal(Rc::new(move |_, _| {
            if webview_works {
                Ok(Box::new(Stub(l2.clone())) as Box<dyn PreviewSurface>)
            } else {
                Err("The preview couldn't start (no WebView).".into())
            }
        })));
    });
    let fake = Arc::new(FakeBackend::with_timing(Timing::instant()).without_media_cache());
    let backend: Arc<dyn Backend> = fake.clone();
    let f2 = fake.clone();
    let (view, cx) = cx.add_window_view(move |window, cx| {
        MainView::new(
            backend,
            Some(f2),
            prefs,
            std::time::Instant::now(),
            window,
            cx,
        )
    });
    cx.run_until_parked();
    (view, fake, log, cx)
}

/// Open the sample thread (quote, TK, video, unresolved quote) in the editor.
fn open_sample(view: &Entity<MainView>, fake: &FakeBackend, cx: &mut VisualTestContext) {
    let id = fake
        .create_draft(Kind::Thread, &crate::fake::studio_sample())
        .unwrap();
    view.update_in(cx, |v, window, cx| {
        let results = v.backend.search("");
        v.list.refresh(results);
        v.open(&id, window, cx);
    });
    cx.run_until_parked();
}

fn frame(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn send(view: &Entity<MainView>, ev: SurfaceEvent, cx: &mut VisualTestContext) {
    let tx = view.read_with(cx, |v, _| v.studio.events.clone());
    tx.try_send(ev).unwrap();
    cx.run_until_parked();
}

fn view_mode(view: &Entity<MainView>, cx: &mut VisualTestContext) -> ViewMode {
    view.read_with(cx, |v, _| v.studio.view)
}

/// Issue #4: a WebView can keep (or drop) the keyboard while the window is
/// in the background; coming back hands it to GPUI so typing works.
#[gpui_kit::test]
fn reactivating_the_window_reclaims_the_keyboard(cx: &mut TestAppContext) {
    let (view, fake, log, cx) = setup(cx, true);
    open_sample(&view, &fake, cx);
    cx.simulate_keystrokes("cmd-2");
    frame(cx);
    let reclaims = |log: &Rc<RefCell<Log>>| {
        log.borrow()
            .0
            .iter()
            .filter(|c| **c == Call::Reclaim)
            .count()
    };
    let before = reclaims(&log);
    cx.deactivate_window();
    assert_eq!(reclaims(&log), before, "nothing while in the background");
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    assert_eq!(reclaims(&log), before + 1);
}

#[gpui_kit::test]
fn mode_keys_switch_panes(cx: &mut TestAppContext) {
    let (view, _, log, cx) = setup(cx, true);
    assert_eq!(view_mode(&view, cx), ViewMode::Write);
    assert!(log.borrow().0.is_empty(), "no WebView until it's needed");

    cx.simulate_keystrokes("cmd-3");
    frame(cx);
    assert_eq!(view_mode(&view, cx), ViewMode::Studio);
    {
        let log = log.borrow();
        assert_eq!(log.loads().len(), 1, "first load is a whole page");
        assert!(log.loads()[0].contains("Content-Security-Policy"));
        assert_eq!(log.visible(), Some(true), "placed over the pane");
        let frame = log.0.iter().find_map(|c| match c {
            Call::Frame(b) => Some(*b),
            _ => None,
        });
        let b = frame.expect("given a frame");
        assert!(b.size.width > gpui_kit::px(100.) && b.size.height > gpui_kit::px(100.));
    }

    // ⌘E in the full editor: the editor alone; again: back.
    cx.simulate_keystrokes("cmd-e");
    frame(cx);
    assert_eq!(view_mode(&view, cx), ViewMode::Focus);
    assert_eq!(log.borrow().visible(), Some(false), "hidden with its pane");
    cx.simulate_keystrokes("cmd-e");
    frame(cx);
    assert_eq!(view_mode(&view, cx), ViewMode::Studio);
    assert_eq!(log.borrow().visible(), Some(true));

    cx.simulate_keystrokes("cmd-2");
    frame(cx);
    assert_eq!(view_mode(&view, cx), ViewMode::Split);
    assert_eq!(log.borrow().visible(), Some(true));

    cx.simulate_keystrokes("cmd-1");
    frame(cx);
    assert_eq!(view_mode(&view, cx), ViewMode::Write);
    assert_eq!(log.borrow().visible(), Some(false));

    // ⌘E from write mode is ⌘2.
    cx.simulate_keystrokes("cmd-e");
    frame(cx);
    assert_eq!(view_mode(&view, cx), ViewMode::Split);
}

#[gpui_kit::test]
fn mode_keys_work_from_the_editor(cx: &mut TestAppContext) {
    let (view, _, _, cx) = setup(cx, true);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.mode), Mode::Edit);
    cx.simulate_keystrokes("cmd-3");
    cx.run_until_parked();
    assert_eq!(view_mode(&view, cx), ViewMode::Studio);
    // Typing still goes to the editor.
    cx.simulate_input("!");
    cx.run_until_parked();
    let text = view.read_with(cx, |v, cx| v.editor.read(cx).value().to_string());
    assert!(text.ends_with('!'), "{text}");
}

#[gpui_kit::test]
fn renders_the_thread_and_patches_after_a_pause(cx: &mut TestAppContext) {
    let (view, fake, log, cx) = setup(cx, true);
    open_sample(&view, &fake, cx);
    cx.simulate_keystrokes("cmd-3");
    frame(cx);
    let page = log.borrow().loads().last().unwrap().to_string();
    assert!(
        page.contains("blockquote class=\"blyg-transclusion\""),
        "{page}"
    );
    assert!(page.contains("A tide pool is a small ocean"));
    assert!(page.contains("class=\"blyg-tk-gen\""), "the TK tint");
    assert!(
        page.contains("figure class=\"blyg-yt\""),
        "the YouTube facade"
    );
    assert!(page.contains("blyg-transclusion unresolved"));
    assert!(page.contains("<article class=\"thread\">"));
    // The mount and base are the blyg's, so links and media resolve.
    assert!(page.contains("<base href=\"https://blyg.example.com/\">"));

    view.read_with(cx, |v, _| {
        let s = v.studio.stats().unwrap();
        assert_eq!((s.quotes, s.ai_spans, s.videos), (1, 1, 1));
        assert_eq!(s.unresolved.len(), 1);
    });

    // Nothing is patched until the page says it's ready.
    send(&view, SurfaceEvent::Ready, cx);
    let before = log.borrow().evals().len();

    // Typing: no render until the pause, then one patch (no reload).
    cx.simulate_input(" Tide tables.");
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(40));
    cx.run_until_parked();
    assert!(
        !log.borrow()
            .evals()
            .iter()
            .skip(before)
            .any(|e| e.contains("patch")),
        "not before the pause"
    );
    cx.executor().advance_clock(Duration::from_millis(120));
    cx.run_until_parked();
    let log_ = log.borrow();
    let patches: Vec<_> = log_
        .evals()
        .into_iter()
        .skip(before)
        .filter(|e| e.contains("__blyg.patch("))
        .collect();
    assert_eq!(patches.len(), 1, "{patches:?}");
    assert!(patches[0].contains("Tide tables."));
    assert_eq!(log_.loads().len(), 1, "patched in place, not reloaded");
}

#[gpui_kit::test]
fn another_item_or_kind_reloads_the_page(cx: &mut TestAppContext) {
    let (view, fake, log, cx) = setup(cx, true);
    cx.simulate_keystrokes("cmd-2");
    frame(cx);
    send(&view, SurfaceEvent::Ready, cx);
    assert_eq!(log.borrow().loads().len(), 1);
    open_sample(&view, &fake, cx);
    frame(cx);
    assert_eq!(log.borrow().loads().len(), 2, "another item: a new page");
    send(&view, SurfaceEvent::Ready, cx);
    cx.simulate_keystrokes("cmd-t");
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(150));
    cx.run_until_parked();
    let log = log.borrow();
    assert_eq!(log.loads().len(), 3, "thread → fragment: a new page");
    assert!(log.loads()[2].contains("<article class=\"fragment\">"));
    // In a fragment, `![[id]]` is literal text.
    assert!(!log.loads()[2].contains("<blockquote class=\"blyg-transclusion"));
}

#[gpui_kit::test]
fn clicking_a_block_moves_the_caret_and_the_caret_scrolls_the_preview(cx: &mut TestAppContext) {
    let (view, fake, log, cx) = setup(cx, true);
    open_sample(&view, &fake, cx);
    cx.simulate_keystrokes("cmd-3");
    frame(cx);
    send(&view, SurfaceEvent::Ready, cx);

    // The quote is on source line 4.
    send(&view, SurfaceEvent::JumpToLine(4), cx);
    view.read_with(cx, |v, cx| {
        let pos = v.editor.read(cx).cursor_position();
        assert_eq!((pos.line, pos.character), (4, 0));
        assert_eq!(v.mode, Mode::Edit);
    });
    assert!(
        log.borrow().0.contains(&Call::FocusParent),
        "keyboard back to GPUI"
    );

    // Moving the caret to the video's line scrolls the preview to its block.
    let line_map = view.read_with(cx, |v, _| {
        v.studio.latest.as_ref().unwrap().1.line_map.clone()
    });
    let video_line = crate::fake::studio_sample()
        .lines()
        .position(|l| l.starts_with("https://www.youtube.com"))
        .unwrap();
    let block = super::Studio::block_for_line(&line_map, video_line).unwrap();
    view.update_in(cx, |v, window, cx| {
        v.editor.update(cx, |s, cx| {
            s.set_cursor_position(
                gpui_kit::base::input::Position {
                    line: video_line as u32,
                    character: 3,
                },
                window,
                cx,
            )
        })
    });
    cx.run_until_parked();
    let want = super::webview::scroll_js(block);
    assert!(
        log.borrow().evals().contains(&want.as_str()),
        "{want} in {:?}",
        log.borrow().evals()
    );
}

#[gpui_kit::test]
fn a_sheet_hides_the_native_view(cx: &mut TestAppContext) {
    let (view, _, log, cx) = setup(cx, true);
    cx.simulate_keystrokes("enter");
    cx.simulate_keystrokes("cmd-2");
    frame(cx);
    assert_eq!(log.borrow().visible(), Some(true));
    cx.simulate_keystrokes("cmd-enter");
    frame(cx);
    assert!(view.read_with(cx, |v, _| matches!(v.sheet, Some(Sheet::Publish { .. }))));
    assert_eq!(log.borrow().visible(), Some(false));
    cx.simulate_keystrokes("escape");
    frame(cx);
    assert_eq!(log.borrow().visible(), Some(true));
}

#[gpui_kit::test]
fn links_open_in_the_browser_not_the_view(cx: &mut TestAppContext) {
    let (view, _, _, cx) = setup(cx, true);
    cx.simulate_keystrokes("cmd-2");
    frame(cx);
    send(
        &view,
        SurfaceEvent::OpenUrl("https://example.com/tides".into()),
        cx,
    );
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://example.com/tides")
    );
}

#[gpui_kit::test]
fn falls_back_to_a_message_without_a_webview(cx: &mut TestAppContext) {
    let (view, fake, _, cx) = setup(cx, false);
    open_sample(&view, &fake, cx);
    cx.simulate_keystrokes("cmd-3");
    frame(cx);
    view.read_with(cx, |v, _| {
        assert_eq!(
            v.studio.failed(),
            Some("The preview couldn't start (no WebView).")
        );
        // The counts still work.
        assert_eq!(v.studio.stats().unwrap().quotes, 1);
    });
}

#[gpui_kit::test]
fn publish_warns_about_unresolved_quotes(cx: &mut TestAppContext) {
    let (view, fake, _, cx) = setup(cx, true);
    open_sample(&view, &fake, cx);
    let w = view.read_with(cx, |v, cx| v.studio_publish_warning(cx));
    assert_eq!(w.as_deref(), Some("⚠ 1 quote can't be resolved"));
    // With only the resolvable quote left, no warning.
    view.update_in(cx, |v, window, cx| {
        let text = format!("Tides\n\n![[{SAMPLE_QUOTE_ID}]]\n");
        v.editor.update(cx, |s, cx| s.set_value(text, window, cx));
    });
    let w = view.read_with(cx, |v, cx| v.studio_publish_warning(cx));
    assert_eq!(w, None);
}
