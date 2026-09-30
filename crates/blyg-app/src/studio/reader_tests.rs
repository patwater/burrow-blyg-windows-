//! Headless GPUI tests of the Reading screen's body: someone else's post goes
//! to the WebView surface (a recording stub) as its blyg published it, the
//! version pill swaps it, Markdown-only items fall back to blyg-render, and
//! only one WebView is ever on screen.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use blyg_core::config::MemoryTokenStore;
use blyg_core::{Backend, ConfigStore, Kind, LocalId, ReadingItem};
use gpui_kit::{Bounds, Entity, Pixels, TestAppContext, VisualTestContext};

use super::super::reading::View;
use super::MainView;
use super::webview::{FactoryGlobal, PreviewSurface};
use crate::fake::reading_seed::*;
use crate::fake::{FakeBackend, Timing};
use crate::prefs::Prefs;

#[derive(Debug, Clone, PartialEq)]
enum Call {
    Frame(Bounds<Pixels>),
    Visible(bool),
    Load(String),
    Eval(String),
    FocusParent,
    Dark(bool),
}

/// Every call, tagged with the surface (in creation order).
#[derive(Default)]
struct Log(Vec<(usize, Call)>, usize);

struct Stub(usize, Rc<RefCell<Log>>);

impl Stub {
    fn push(&self, c: Call) {
        self.1.borrow_mut().0.push((self.0, c));
    }
}

impl PreviewSurface for Stub {
    fn set_frame(&mut self, b: Bounds<Pixels>) {
        self.push(Call::Frame(b));
    }
    fn set_visible(&mut self, v: bool) {
        self.push(Call::Visible(v));
    }
    fn load(&mut self, html: &str) {
        self.push(Call::Load(html.to_string()));
    }
    fn eval(&mut self, js: &str) {
        self.push(Call::Eval(js.to_string()));
    }
    fn focus_parent(&mut self) {
        self.push(Call::FocusParent);
    }
    fn set_dark(&mut self, dark: bool) {
        self.push(Call::Dark(dark));
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Which {
    Studio,
    Reader,
}

impl Log {
    /// Which surface an id is, from the policy of the first page it loaded
    /// (the reader's CSP admits images over https only).
    fn which(&self, id: usize) -> Option<Which> {
        self.0.iter().find_map(|(i, c)| match c {
            Call::Load(h) if *i == id => Some(if h.contains("img-src https: data:;") {
                Which::Reader
            } else {
                Which::Studio
            }),
            _ => None,
        })
    }
    fn ids(&self, w: Which) -> Vec<usize> {
        (0..self.1).filter(|i| self.which(*i) == Some(w)).collect()
    }
    fn pages(&self, w: Which) -> Vec<String> {
        let ids = self.ids(w);
        self.0
            .iter()
            .filter_map(|(i, c)| match c {
                Call::Load(h) if ids.contains(i) => Some(h.clone()),
                _ => None,
            })
            .collect()
    }
    fn visible(&self, w: Which) -> bool {
        let ids = self.ids(w);
        self.0
            .iter()
            .rev()
            .find_map(|(i, c)| match c {
                Call::Visible(v) if ids.contains(i) => Some(*v),
                _ => None,
            })
            .unwrap_or(false)
    }
    fn evals(&self) -> usize {
        self.0
            .iter()
            .filter(|(_, c)| matches!(c, Call::Eval(_)))
            .count()
    }
}

type Setup<'a> = (
    Entity<MainView>,
    Arc<FakeBackend>,
    Rc<RefCell<Log>>,
    &'a mut VisualTestContext,
);

fn setup(cx: &mut TestAppContext) -> Setup<'_> {
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
            let id = {
                let mut l = l2.borrow_mut();
                l.1 += 1;
                l.1 - 1
            };
            Ok(Box::new(Stub(id, l2.clone())) as Box<dyn PreviewSurface>)
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

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..20 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn open_row(view: &Entity<MainView>, id: &str, cx: &mut VisualTestContext) {
    let key = view.read_with(cx, |v, _| {
        v.reading
            .rows
            .iter()
            .find(|r| r.remote_id == id)
            .map(super::super::reading::vm::key)
            .expect("row shown")
    });
    view.update_in(cx, |v, window, cx| v.open_reading(key, window, cx));
    settle(cx);
}

fn reading(view: &Entity<MainView>, cx: &mut VisualTestContext) {
    cx.simulate_keystrokes("cmd-r");
    settle(cx);
    assert_eq!(view.read_with(cx, |v, _| v.reading.view), View::Reading);
}

fn last(log: &Rc<RefCell<Log>>, w: Which) -> String {
    log.borrow().pages(w).last().cloned().expect("a page")
}

#[gpui_kit::test]
fn the_body_is_the_published_html_in_the_webview(cx: &mut TestAppContext) {
    let (view, _, log, cx) = setup(cx);
    reading(&view, cx);
    open_row(&view, LIN_GARDENS, cx);
    let page = last(&log, Which::Reader);
    // The transclusion snapshot and the image, exactly as Lin's blyg baked
    // them (not `![[…]]` as literal text).
    assert!(
        page.contains(
            "<blockquote class=\"blyg-transclusion\" data-blyg-id=\"01K2ADA0TIDES0000000000001\""
        ),
        "{page}"
    );
    assert!(page.contains("Tide tables are a kind of promise"));
    assert!(!page.contains("![["), "{page}");
    assert!(
        page.contains(
            "<img src=\"media/raised-beds.jpg\" alt=\"Raised beds in October\" \
             referrerpolicy=\"no-referrer\">"
        ),
        "{page}"
    );
    // Relative URLs resolve against the author's origin.
    assert!(page.contains("<base href=\"https://lin.blyg.example.com/\">"));
    // The attachment from the item document follows the text.
    assert!(page.contains("src=\"https://lin.blyg.example.com/media/seed-packets.jpg\""));
    assert!(page.find("Cutting back").unwrap() < page.find("seed-packets").unwrap());
    // Paragraphs and headings are real HTML, in the app's reader theme.
    assert!(page.contains("<h2>Pruning</h2>"));
    assert!(page.contains("<article class=\"thread\">"));
    assert!(page.contains("font-family: \"Literata\""));
    view.read_with(cx, |v, _| {
        assert!(v.studio.reader.active());
        assert!(v.studio.reader.visible());
        assert_eq!(v.studio.reader.pages.last(), Some(&page));
    });
    assert!(log.borrow().visible(Which::Reader));
    assert!(!log.borrow().visible(Which::Studio));

    // The same post on the next frame: no reload.
    let n = log.borrow().pages(Which::Reader).len();
    settle(cx);
    assert_eq!(log.borrow().pages(Which::Reader).len(), n);
    // Clicks in the page never navigate it; nothing is patched in.
    assert_eq!(log.borrow().evals(), 0);
}

#[gpui_kit::test]
fn stepping_the_pill_swaps_the_body(cx: &mut TestAppContext) {
    let (view, _, log, cx) = setup(cx);
    reading(&view, cx);
    open_row(&view, RUE_TRUST, cx);
    let current = last(&log, Which::Reader);
    assert!(current.contains("Spend it carefully."), "{current}");

    // ‹ : pinned v3, from its public document.
    cx.simulate_keystrokes("left");
    settle(cx);
    let pinned = last(&log, Which::Reader);
    assert!(
        pinned.contains("A hyperlink is a small unit of trust.</p>"),
        "{pinned}"
    );
    assert!(!pinned.contains("Spend it carefully."));

    // "Diff vs now" is native: the WebView steps aside.
    view.update_in(cx, |v, window, cx| {
        v.reading_action_for_test("Diff vs now", window, cx)
    });
    settle(cx);
    assert!(view.read_with(cx, |v, _| !v.studio.reader.active()));
    assert!(!log.borrow().visible(Which::Reader));
    view.update_in(cx, |v, window, cx| {
        v.reading_action_for_test("Diff vs now", window, cx)
    });
    settle(cx);
    assert!(log.borrow().visible(Which::Reader));

    // Back to current: the current HTML again.
    view.update_in(cx, |v, window, cx| {
        v.reading_action_for_test("Back to current", window, cx)
    });
    settle(cx);
    assert!(last(&log, Which::Reader).contains("Spend it carefully."));
}

#[gpui_kit::test]
fn markdown_only_items_render_with_blyg_render(cx: &mut TestAppContext) {
    let (view, fake, log, cx) = setup(cx);
    // Omar's RSS item carries only text.
    reading(&view, cx);
    open_row(&view, OMAR_YEAR, cx);
    let page = last(&log, Which::Reader);
    assert!(
        page.contains("<p>The consulting year in review</p>"),
        "{page}"
    );
    assert!(page.contains("<p>Fewer clients, longer projects"));
    assert!(page.contains("<base href=\"https://omar.example.com/\">"));

    // A Markdown-only thread quoting another post held from the same origin:
    // the quote resolves locally, like the author's publish would bake it.
    let quoted = "01k2ada0zzzz0000000000000q";
    let thread = "01k2ada0zzzz0000000000000t";
    fake.with_reading(|rows| {
        let ada = rows
            .iter()
            .find(|r| r.remote_id == ADA_FINISHED)
            .unwrap()
            .clone();
        rows.push(ReadingItem {
            remote_id: quoted.into(),
            content_md: "Harbours are patient.".into(),
            content_html: "<p>Harbours are patient.</p>\n".into(),
            ..ada.clone()
        });
        rows.push(ReadingItem {
            remote_id: thread.into(),
            kind: Kind::Thread,
            content_md: format!("On waiting\n\n![[{quoted}]]\n"),
            content_html: String::new(),
            ..ada
        });
    });
    settle(cx);
    open_row(&view, thread, cx);
    let page = last(&log, Which::Reader);
    assert!(
        page.contains(&format!(
            "<blockquote class=\"blyg-transclusion\" data-blyg-id=\"{quoted}\""
        )),
        "{page}"
    );
    assert!(page.contains("Harbours are patient."), "{page}");
    assert!(!page.contains("![["), "{page}");
}

#[gpui_kit::test]
fn one_webview_on_screen_at_a_time(cx: &mut TestAppContext) {
    let (view, _, log, cx) = setup(cx);
    // The studio preview beside the editor.
    view.update_in(cx, |v, window, cx| {
        v.open(&LocalId("01J9M2A".into()), window, cx)
    });
    cx.simulate_keystrokes("cmd-2");
    settle(cx);
    assert!(log.borrow().visible(Which::Studio));
    assert!(!log.borrow().visible(Which::Reader));

    // ⌘Y: the history's current version is in the reader; the studio hides.
    cx.simulate_keystrokes("cmd-y");
    settle(cx);
    assert!(view.read_with(cx, |v, _| v.reading.own.is_some()));
    assert!(log.borrow().visible(Which::Reader));
    assert!(!log.borrow().visible(Which::Studio));
    let own = last(&log, Which::Reader);
    assert!(own.contains("every extra step between noticing"), "{own}");
    assert!(own.contains("<base href=\"https://blyg.example.com/\">"));

    // An older version has no text on screen: the reader hides.
    view.update_in(cx, |v, _, cx| {
        v.reading.own.as_mut().unwrap().sel = Some(1);
        cx.notify();
    });
    settle(cx);
    assert!(!log.borrow().visible(Which::Reader));
    assert!(!log.borrow().visible(Which::Studio));

    // ⌘Y again closes the history: the studio comes back, the reader hides.
    cx.simulate_keystrokes("cmd-y");
    settle(cx);
    assert!(view.read_with(cx, |v, _| v.reading.own.is_none()));
    assert!(log.borrow().visible(Which::Studio));
    assert!(!log.borrow().visible(Which::Reader));

    // The Reading screen: only the reader.
    reading(&view, cx);
    open_row(&view, LIN_GARDENS, cx);
    assert!(log.borrow().visible(Which::Reader));
    assert!(!log.borrow().visible(Which::Studio));

    // The version dropdown and sheets are GPUI: the WebView hides under them.
    cx.simulate_keystrokes("cmd-y");
    settle(cx);
    assert!(view.read_with(cx, |v, _| v.reading.opened.as_ref().unwrap().dropdown));
    assert!(!log.borrow().visible(Which::Reader));
    cx.simulate_keystrokes("cmd-y");
    settle(cx);
    assert!(log.borrow().visible(Which::Reader));
    // The profile sheet (⌘I) slides over the body: the WebView hides.
    cx.simulate_keystrokes("cmd-i");
    settle(cx);
    assert!(view.read_with(cx, |v, _| v.profile_sheet_open()));
    assert!(
        !log.borrow().visible(Which::Reader),
        "hidden under the profile"
    );
    cx.simulate_keystrokes("escape");
    settle(cx);
    assert!(log.borrow().visible(Which::Reader));
    cx.simulate_keystrokes("cmd-,");
    settle(cx);
    assert!(view.read_with(cx, |v, _| v.sheet.is_some()));
    assert!(!log.borrow().visible(Which::Reader));
    assert!(!log.borrow().visible(Which::Studio));

    // Every Visible(true) of one surface came while the other was hidden.
    let log = log.borrow();
    let (studio, reader) = (log.ids(Which::Studio), log.ids(Which::Reader));
    let (mut s_on, mut r_on) = (false, false);
    for (id, c) in &log.0 {
        if let Call::Visible(v) = c {
            if studio.contains(id) {
                s_on = *v;
            } else if reader.contains(id) {
                r_on = *v;
            }
            assert!(!(s_on && r_on), "both WebViews on screen");
        }
    }
}

#[gpui_kit::test]
fn a_withdrawn_post_without_a_pin_shows_no_body(cx: &mut TestAppContext) {
    let (view, _, log, cx) = setup(cx);
    reading(&view, cx);
    // Withdrawn, but its pinned v2 is retained: shown, attributed.
    open_row(&view, RUE_KEPT, cx);
    assert!(last(&log, Which::Reader).contains("Every archive is an argument"));
    view.update_in(cx, |v, window, cx| {
        let o = v.reading.opened.as_mut().unwrap();
        o.item.pinned_version_retained = None;
        o.item.content_md.clear();
        o.item.content_html.clear();
        let _ = window;
        cx.notify();
    });
    settle(cx);
    assert!(view.read_with(cx, |v, _| !v.studio.reader.active()));
    assert!(!log.borrow().visible(Which::Reader));
}
