//! --- notes --- The text the drawer appends, and the drawer on a
//! zero-latency FakeBackend: toggling, esc, "→ Notes" from posts and the
//! browser, selections, the web views it cuts off, and the scratch note
//! behind it.

use super::{
    MAX_QUOTE, PostRef, TITLE, append, fresh_title, is_notes_title, link, page_seed,
    parse_selection, post_block, quote_with_source,
};

// ------------------------------------------------------------ formatting

#[test]
fn links_escape_brackets_and_wrap_parentheses() {
    assert_eq!(
        link("Tide tables", "https://blyg.example.com/f/abc/"),
        "[Tide tables](https://blyg.example.com/f/abc/)"
    );
    assert_eq!(
        link(
            "On [draft] notes\nand more",
            "https://blyg.example.com/wiki/Tide_(sea)"
        ),
        "[On \\[draft\\] notes and more](<https://blyg.example.com/wiki/Tide_(sea)>)"
    );
    assert_eq!(
        link("a \\ b", "https://blyg.example.com/"),
        "[a \\\\ b](https://blyg.example.com/)"
    );
    // No title: the URL is the text.
    assert_eq!(
        link("  ", "https://blyg.example.com/"),
        "[https://blyg.example.com/](https://blyg.example.com/)"
    );
}

#[test]
fn selections_are_quoted_with_their_source() {
    assert_eq!(
        quote_with_source(
            "  first line\n\nsecond line  ",
            "Gardens [1]",
            Some("https://lin.blyg.example.com/f/g/")
        ),
        "> first line\n>\n> second line\n>\n> — [Gardens \\[1\\]](https://lin.blyg.example.com/f/g/)"
    );
    assert_eq!(quote_with_source("just this", "", None), "> just this");
    assert_eq!(
        quote_with_source("x", "Rue", None),
        "> x\n>\n> — Rue",
        "no URL: the name alone"
    );
    // A page can't flood the note.
    let long = "a".repeat(MAX_QUOTE + 500);
    assert_eq!(
        quote_with_source(&long, "", None).len(),
        2 + MAX_QUOTE,
        "\"> \" + the first MAX_QUOTE characters"
    );
}

#[test]
fn posts_become_a_quote_or_a_link() {
    let blyg = PostRef {
        blyg_id: Some("01K2RUE0TRUST0000000000001".into()),
        title: "Rue".into(),
        first_line: "A hyperlink is a small unit of trust.".into(),
        url: Some("https://rue.blyg.example.com/f/trust/".into()),
    };
    // A thread may hold quotes: the transclusion.
    assert_eq!(post_block(&blyg, true), "![[01K2RUE0TRUST0000000000001]]");
    // A fragment can't: a link and the first line, quoted.
    assert_eq!(
        post_block(&blyg, false),
        "[Rue](https://rue.blyg.example.com/f/trust/)\n> A hyperlink is a small unit of trust."
    );
    // A feed post: the link only.
    let rss = PostRef {
        blyg_id: None,
        title: "The year [in review]".into(),
        first_line: "Fewer clients.".into(),
        url: Some("https://omar.example.com/2026/year".into()),
    };
    assert_eq!(
        post_block(&rss, true),
        "[The year \\[in review\\]](https://omar.example.com/2026/year)"
    );
    // The first line isn't repeated when it's the title.
    let short = PostRef {
        first_line: "Rue".into(),
        ..blyg
    };
    assert_eq!(
        post_block(&short, false),
        "[Rue](https://rue.blyg.example.com/f/trust/)"
    );
}

#[test]
fn appending_adds_a_paragraph_and_puts_the_caret_below() {
    let (t, c) = append("", "![[a]]");
    assert_eq!(t, "![[a]]\n\n");
    assert_eq!(c, t.len());
    let (t, c) = append("# Reading notes\n\nsome thought  \n\n\n", "![[a]]");
    assert_eq!(t, "# Reading notes\n\nsome thought\n\n![[a]]\n\n");
    assert_eq!(c, t.len());
    let (t2, _) = append(&t, "[b](https://blyg.example.com/)");
    assert_eq!(
        t2,
        "# Reading notes\n\nsome thought\n\n![[a]]\n\n[b](https://blyg.example.com/)\n\n"
    );
}

#[test]
fn pages_and_titles() {
    assert_eq!(page_seed(TITLE), "# Reading notes\n\n");
    let d = chrono::NaiveDate::from_ymd_opt(2026, 9, 7).unwrap();
    assert_eq!(fresh_title(d), "Reading notes · Sep 7");
    assert!(is_notes_title("Reading notes · Sep 7"));
    assert!(!is_notes_title("Notes on reading"));
    assert_eq!(parse_selection("\"a \\\"b\\\"\""), "a \"b\"");
    assert_eq!(parse_selection("null"), "");
    assert!(super::SELECTION_JS.contains("window.getSelection()"));
}

/// A passage selected in the notes, for universal quoting: markers dropped,
/// and the source of the quote it sits in (a note has none of its own).
#[test]
fn a_notes_passage_keeps_its_quotes_source() {
    use super::notes_passage;
    let text = "# Reading notes\n\nMy own thought.\n\n\
                > High water at 6:12\n>\n> — [Tide tables \\[2026\\]](https://blyg.example.com/f/abc/)\n\nMore.";
    let at = |s: &str| {
        let i = text.find(s).unwrap();
        i..i + s.len()
    };
    // Own words: no link.
    assert_eq!(
        notes_passage(text, at("own thought")),
        Some(("own thought".into(), None))
    );
    // Inside a quote from the browser: the passage and that page.
    let want = Some((
        "Tide tables [2026]".to_string(),
        "https://blyg.example.com/f/abc/".to_string(),
    ));
    assert_eq!(
        notes_passage(text, at("water at")),
        Some(("water at".into(), want.clone()))
    );
    // The whole quote selected: markers and the source line dropped.
    let whole = at(
        "> High water at 6:12\n>\n> — [Tide tables \\[2026\\]](https://blyg.example.com/f/abc/)",
    );
    let (p, link) = notes_passage(text, whole).unwrap();
    assert_eq!(p.trim(), "High water at 6:12");
    assert_eq!(link, want);
    // Nothing selected.
    assert_eq!(notes_passage(text, 3..3), None);
}

// ------------------------------------------------------------ the drawer

mod ui {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;

    use blyg_core::config::MemoryTokenStore;
    use blyg_core::{Backend, ConfigStore, Kind, Status};
    use gpui_kit::{Bounds, Entity, Pixels, TestAppContext, VisualTestContext, px};

    use crate::app::MainView;
    use crate::app::browser::surface::{BrowserSurface, FactoryGlobal, PageState};
    use crate::app::reading::View;
    use crate::app::reading::stream_vm::ReadMode;
    use crate::app::studio::webview::{self, PreviewSurface};
    use crate::fake::reading_seed::*;
    use crate::fake::{FakeBackend, Timing};
    use crate::prefs::Prefs;

    /// What the stubs record, and the selection they answer with.
    #[derive(Default)]
    struct Log {
        selection: String,
        browser_frames: Vec<Bounds<Pixels>>,
        browser_loads: Vec<String>,
        reader_frames: Vec<Bounds<Pixels>>,
        reader_visible: bool,
        /// Scripts run in the reader's page.
        reader_evals: Vec<String>,
        /// The browser's page has the keyboard (it was clicked).
        page_keyboard: bool,
    }

    struct Browser(Rc<RefCell<Log>>);

    impl BrowserSurface for Browser {
        fn set_frame(&mut self, b: Bounds<Pixels>) {
            self.0.borrow_mut().browser_frames.push(b);
        }
        fn set_visible(&mut self, _: bool) {}
        fn load_url(&mut self, url: &str) {
            self.0.borrow_mut().browser_loads.push(url.to_string());
        }
        fn back(&mut self) {}
        fn forward(&mut self) {}
        fn reload(&mut self) {}
        fn stop(&mut self) {}
        fn state(&self) -> PageState {
            PageState {
                url: self
                    .0
                    .borrow()
                    .browser_loads
                    .last()
                    .cloned()
                    .unwrap_or_default(),
                title: "Tide tables [2026]".into(),
                ..PageState::default()
            }
        }
        fn refresh_blocking(&mut self) {}
        /// As WebKit does: a page that loses the keyboard loses its
        /// selection.
        fn focus_parent(&mut self) {
            let mut l = self.0.borrow_mut();
            l.page_keyboard = false;
            l.selection.clear();
        }
        fn page_has_keyboard(&self) -> bool {
            self.0.borrow().page_keyboard
        }
        fn selection(&mut self, reply: async_channel::Sender<String>) {
            let _ = reply.try_send(self.0.borrow().selection.clone());
        }
    }

    struct Preview(Rc<RefCell<Log>>);

    impl PreviewSurface for Preview {
        fn set_frame(&mut self, b: Bounds<Pixels>) {
            self.0.borrow_mut().reader_frames.push(b);
        }
        fn set_visible(&mut self, v: bool) {
            self.0.borrow_mut().reader_visible = v;
        }
        fn load(&mut self, _: &str) {}
        fn eval(&mut self, js: &str) {
            self.0.borrow_mut().reader_evals.push(js.to_string());
        }
        fn focus_parent(&mut self) {}
        fn set_dark(&mut self, _: bool) {}
        fn selection(&mut self, reply: async_channel::Sender<String>) {
            let _ = reply.try_send(self.0.borrow().selection.clone());
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
        let (l1, l2) = (log.clone(), log.clone());
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::app::bind_keys(cx);
            crate::settings::init(
                ConfigStore::in_memory(&config),
                Arc::new(MemoryTokenStore::default()),
                None,
                cx,
            );
            cx.set_global(FactoryGlobal(Rc::new(move |_, _, _, _| {
                Ok(Box::new(Browser(l1.clone())) as Box<dyn BrowserSurface>)
            })));
            cx.set_global(webview::FactoryGlobal(Rc::new(move |_, _| {
                Ok(Box::new(Preview(l2.clone())) as Box<dyn PreviewSurface>)
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
        for _ in 0..10 {
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }

    /// Let the slide-out finish.
    fn slide_out(cx: &mut VisualTestContext) {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(
                crate::app::notes::SLIDE_MS + 50,
            ));
        settle(cx);
    }

    fn reading(view: &Entity<MainView>, mode: ReadMode, cx: &mut VisualTestContext) {
        view.update_in(cx, |v, window, cx| {
            v.reading.mode = mode;
            v.show_view(View::Reading, window, cx);
        });
        settle(cx);
    }

    fn key_of(
        view: &Entity<MainView>,
        id: &str,
        cx: &mut VisualTestContext,
    ) -> crate::app::reading::vm::Key {
        view.read_with(cx, |v, _| {
            v.reading
                .rows
                .iter()
                .find(|r| r.remote_id == id)
                .map(crate::app::reading::vm::key)
                .expect("row")
        })
    }

    fn notes_text(view: &Entity<MainView>, cx: &mut VisualTestContext) -> String {
        view.read_with(cx, |v, cx| v.notes.text(cx))
    }

    fn is_open(view: &Entity<MainView>, cx: &mut VisualTestContext) -> bool {
        view.read_with(cx, |v, _| v.notes.open)
    }

    /// The scratch note behind the drawer, from the store.
    fn stored(
        view: &Entity<MainView>,
        fake: &FakeBackend,
        cx: &mut VisualTestContext,
    ) -> blyg_core::Item {
        let id = view
            .read_with(cx, |v, _| v.notes.id.clone())
            .expect("a note");
        fake.item(&id).expect("in the store")
    }

    #[gpui_kit::test]
    fn cmd_shift_n_toggles_and_esc_closes(cx: &mut TestAppContext) {
        let (view, fake, _, cx) = setup(cx);
        let before = fake.items().len();
        cx.simulate_keystrokes(&crate::keymap::keys("cmd-shift-n"));
        settle(cx);
        assert!(is_open(&view, cx));
        assert!(cx.debug_bounds("notes-drawer").is_some(), "drawn");
        assert_eq!(notes_text(&view, cx), "# Reading notes\n\n");
        assert_eq!(fake.items().len(), before, "nothing kept until you write");
        // Typing goes to the drawer.
        cx.simulate_input("benches, again");
        settle(cx);
        assert_eq!(notes_text(&view, cx), "# Reading notes\n\nbenches, again");
        // ⇧⌘N again: slides back (still drawn while it slides).
        cx.simulate_keystrokes(&crate::keymap::keys("cmd-shift-n"));
        settle(cx);
        assert!(!is_open(&view, cx));
        slide_out(cx);
        assert!(cx.debug_bounds("notes-drawer").is_none(), "gone");
        // esc with the drawer's editor focused closes it too.
        cx.simulate_keystrokes(&crate::keymap::keys("cmd-shift-n"));
        settle(cx);
        assert!(is_open(&view, cx));
        cx.simulate_keystrokes("escape");
        settle(cx);
        assert!(!is_open(&view, cx));
        // The menu item's action is the same.
        cx.dispatch_action(crate::app::notes::ToggleNotes);
        settle(cx);
        assert!(is_open(&view, cx));
    }

    #[gpui_kit::test]
    fn the_notes_are_a_real_scratch_note(cx: &mut TestAppContext) {
        let (view, fake, _, cx) = setup(cx);
        cx.simulate_keystrokes(&crate::keymap::keys("cmd-shift-n"));
        settle(cx);
        cx.simulate_input("a thought about gardens");
        settle(cx);
        let it = stored(&view, &fake, cx);
        assert_eq!(it.status, Status::Scratch);
        assert_eq!(it.kind, Kind::Thread, "so ![[…]] quotes are valid");
        assert_eq!(it.title(), "Reading notes");
        assert_eq!(it.content_md, "# Reading notes\n\na thought about gardens");
        assert!(!it.pending_sync, "local only");
        // In the Posts list and searchable, like any scratch note.
        assert!(
            fake.search("thought about gardens")
                .iter()
                .any(|i| i.local_id == it.local_id)
        );
        // Closed and reopened: the same note, the same text.
        cx.simulate_keystrokes("escape");
        slide_out(cx);
        cx.simulate_keystrokes(&crate::keymap::keys("cmd-shift-n"));
        settle(cx);
        assert_eq!(stored(&view, &fake, cx).local_id, it.local_id);
        assert_eq!(notes_text(&view, cx), it.content_md);
    }

    #[gpui_kit::test]
    fn a_new_page_leaves_the_old_one_in_posts(cx: &mut TestAppContext) {
        let (view, fake, _, cx) = setup(cx);
        cx.simulate_keystrokes(&crate::keymap::keys("cmd-shift-n"));
        cx.simulate_input("first page");
        settle(cx);
        let first = stored(&view, &fake, cx).local_id;
        view.update_in(cx, |v, window, cx| v.notes_new_page(window, cx));
        settle(cx);
        let second = stored(&view, &fake, cx);
        assert_ne!(second.local_id, first);
        assert!(second.title().starts_with("Reading notes · "));
        assert_eq!(second.status, Status::Scratch);
        let old = fake.item(&first).expect("still there");
        assert_eq!(old.status, Status::Scratch);
        assert!(old.content_md.ends_with("first page"));
    }

    #[gpui_kit::test]
    fn cmd_d_in_the_drawer_makes_the_notes_a_draft(cx: &mut TestAppContext) {
        let (view, fake, _, cx) = setup(cx);
        // Something else open in the editor: ⌘D must not touch it.
        let other = fake.create_scratch(Kind::Fragment, "not these").unwrap();
        view.update_in(cx, |v, window, cx| v.open(&other, window, cx));
        cx.simulate_keystrokes(&crate::keymap::keys("cmd-shift-n"));
        cx.simulate_input("draft me");
        settle(cx);
        let id = stored(&view, &fake, cx).local_id;
        cx.simulate_keystrokes(&crate::keymap::keys("cmd-d"));
        settle(cx);
        assert_eq!(fake.item(&id).unwrap().status, Status::Draft);
        assert_eq!(fake.item(&other).unwrap().status, Status::Scratch);
        view.read_with(cx, |v, _| assert!(v.notes.id.is_none(), "a new page"));
        assert_eq!(notes_text(&view, cx), "# Reading notes\n\n");
    }

    #[gpui_kit::test]
    fn notes_on_a_blyg_post_adds_its_quote(cx: &mut TestAppContext) {
        let (view, fake, _, cx) = setup(cx);
        reading(&view, ReadMode::Stream, cx);
        let key = key_of(&view, RUE_TRUST, cx);
        view.update_in(cx, |v, window, cx| {
            v.stream_action_for_test(key, "Notes", window, cx)
        });
        settle(cx);
        assert!(is_open(&view, cx));
        let text = notes_text(&view, cx);
        assert_eq!(text, format!("# Reading notes\n\n![[{RUE_TRUST}]]\n\n"));
        assert_eq!(stored(&view, &fake, cx).content_md, text, "saved");
        // The caret is on a new line below, ready to type.
        cx.simulate_input("why this matters");
        settle(cx);
        assert_eq!(
            notes_text(&view, cx),
            format!("# Reading notes\n\n![[{RUE_TRUST}]]\n\nwhy this matters")
        );
        // The chip is in the stream's action row.
        view.update_in(cx, |v, window, cx| v.close_notes(window, cx));
        slide_out(cx);
        view.update_in(cx, |v, window, cx| v.stream_move(1, window, cx));
        settle(cx);
        assert!(cx.debug_bounds("stream-act-Notes").is_some());
    }

    #[gpui_kit::test]
    fn notes_on_a_feed_post_adds_a_link(cx: &mut TestAppContext) {
        let (view, _, _, cx) = setup(cx);
        reading(&view, ReadMode::Stream, cx);
        let key = key_of(&view, OMAR_YEAR, cx);
        view.update_in(cx, |v, window, cx| {
            v.stream_action_for_test(key, "Notes", window, cx)
        });
        settle(cx);
        let text = notes_text(&view, cx);
        assert!(!text.contains("![["), "{text}");
        assert!(
            text.contains("[The consulting year in review](https://omar.example.com/"),
            "{text}"
        );
    }

    #[gpui_kit::test]
    fn a_fragment_note_gets_links_not_quotes(cx: &mut TestAppContext) {
        let (view, fake, _, cx) = setup(cx);
        cx.simulate_keystrokes(&crate::keymap::keys("cmd-shift-n"));
        cx.simulate_input("x");
        settle(cx);
        let id = stored(&view, &fake, cx).local_id;
        fake.set_kind(&id, Kind::Fragment).unwrap();
        reading(&view, ReadMode::Stream, cx);
        let key = key_of(&view, RUE_TRUST, cx);
        view.update_in(cx, |v, window, cx| {
            v.stream_action_for_test(key, "Notes", window, cx)
        });
        settle(cx);
        let text = notes_text(&view, cx);
        assert!(!text.contains("![["), "{text}");
        assert!(text.contains("](https://rue.blyg.example.com/"), "{text}");
    }

    #[gpui_kit::test]
    fn the_reader_row_has_notes_and_quotes_a_selection(cx: &mut TestAppContext) {
        let (view, _, log, cx) = setup(cx);
        reading(&view, ReadMode::Reader, cx);
        let key = key_of(&view, LIN_GARDENS, cx);
        view.update_in(cx, |v, window, cx| v.open_reading(key, window, cx));
        settle(cx);
        assert!(view.read_with(cx, |v, _| v.studio.reader.active()));
        assert!(
            view.read_with(cx, |v, _| v
                .reading_action_chips()
                .iter()
                .any(|c| c.id == "Notes" && c.label == "→ Notes" && c.enabled)),
            "in the action row"
        );
        // Nothing selected: the post.
        view.update_in(cx, |v, window, cx| {
            v.reading_action_for_test("Notes", window, cx)
        });
        settle(cx);
        assert!(notes_text(&view, cx).contains(&format!("![[{LIN_GARDENS}]]")));
        view.update_in(cx, |v, window, cx| v.close_notes(window, cx));
        slide_out(cx);
        // A passage selected in the post: ⇧⌘N quotes it with a link.
        log.borrow_mut().selection = "Gardens, not streams".into();
        view.update_in(cx, |v, window, cx| {
            v.toggle_notes(&crate::app::notes::ToggleNotes, window, cx)
        });
        settle(cx);
        assert!(is_open(&view, cx));
        let text = notes_text(&view, cx);
        assert!(
            text.contains("> Gardens, not streams\n>\n> — [")
                && text.contains("](https://lin.blyg.example.com/"),
            "{text}"
        );
    }

    #[gpui_kit::test]
    fn the_browsers_notes_button_adds_the_page(cx: &mut TestAppContext) {
        let (view, _, log, cx) = setup(cx);
        view.update_in(cx, |v, window, cx| {
            v.open_url_in_app(
                "https://blyg.example.com/f/abc/",
                crate::app::browser::OpenMode::Slide,
                window,
                cx,
            );
        });
        settle(cx);
        view.update_in(cx, |v, window, cx| {
            v.browser_refresh_state(cx);
            v.browser_send_to_notes(window, cx);
        });
        settle(cx);
        assert!(is_open(&view, cx));
        assert!(
            notes_text(&view, cx)
                .ends_with("[Tide tables \\[2026\\]](https://blyg.example.com/f/abc/)\n\n"),
            "{}",
            notes_text(&view, cx)
        );
        // Not the clipboard any more.
        assert!(cx.read_from_clipboard().is_none());
        // With a selection on the page: quoted, with the page's link.
        log.borrow_mut().selection = "High water at 6:12".into();
        view.update_in(cx, |v, window, cx| v.browser_send_to_notes(window, cx));
        settle(cx);
        assert!(notes_text(&view, cx).ends_with(
            "> High water at 6:12\n>\n> — [Tide tables \\[2026\\]](https://blyg.example.com/f/abc/)\n\n"
        ));
    }

    #[gpui_kit::test]
    fn web_views_are_cut_off_at_the_drawer(cx: &mut TestAppContext) {
        let (view, _, log, cx) = setup(cx);
        view.update_in(cx, |v, window, cx| {
            v.open_url_in_app(
                "https://blyg.example.com/",
                crate::app::browser::OpenMode::Slide,
                window,
                cx,
            );
        });
        settle(cx);
        let full = *log.borrow().browser_frames.last().expect("placed");
        let viewport = cx.update(|window, _| window.viewport_size().width);
        view.update_in(cx, |v, window, cx| v.open_notes(window, cx));
        settle(cx);
        let edge = viewport - px(crate::app::notes::WIDTH);
        let f = *log.borrow().browser_frames.last().unwrap();
        assert!(
            f.origin.x + f.size.width <= edge + px(0.5),
            "{f:?} past {edge:?}"
        );
        assert!(f.size.width < full.size.width);
        // Closed (after the slide): the page gets its width back.
        view.update_in(cx, |v, window, cx| v.close_notes(window, cx));
        slide_out(cx);
        let f = *log.borrow().browser_frames.last().unwrap();
        assert_eq!(f.size.width, full.size.width);
    }

    /// The browser pane's chrome (shield, open, → Notes, close) is never
    /// under the drawer: the pane makes room, in both modes, and gets it
    /// back after.
    #[gpui_kit::test]
    fn the_browser_pane_makes_room_for_the_drawer(cx: &mut TestAppContext) {
        use crate::app::browser::OpenMode;
        let (view, _, _, cx) = setup(cx);
        let viewport = cx.update(|window, _| window.viewport_size().width);
        let edge = viewport - px(crate::app::notes::WIDTH);
        for mode in [OpenMode::Slide, OpenMode::Full] {
            view.update_in(cx, |v, window, cx| {
                v.open_url_in_app("https://blyg.example.com/", mode, window, cx);
            });
            settle(cx);
            let close = cx.debug_bounds("browser-close").expect("chrome");
            assert!(close.right() > edge, "flush right without the drawer");
            view.update_in(cx, |v, window, cx| v.open_notes(window, cx));
            // The pane slides alongside the drawer.
            slide_out(cx);
            let drawer = cx.debug_bounds("notes-drawer").expect("drawer");
            for b in [
                "browser-shield",
                "browser-external",
                "browser-notes",
                "browser-close",
            ] {
                let r = cx.debug_bounds(b).expect("a chrome button");
                assert!(
                    r.right() <= drawer.left() + px(0.5),
                    "{b} {r:?} under {drawer:?}"
                );
            }
            if mode == OpenMode::Full {
                let back = cx.debug_bounds("browser-back").expect("back");
                assert!(back.left() < px(40.), "full mode keeps its left edge");
            }
            view.update_in(cx, |v, window, cx| v.close_notes(window, cx));
            slide_out(cx);
            let close = cx.debug_bounds("browser-close").expect("chrome");
            assert!(close.right() > edge, "the room is given back");
        }
    }

    /// "→ Notes" clicked while the drawer is out isn't a click away: it
    /// adds, and the drawer stays put (no slide out and back in). A click
    /// anywhere else still closes it.
    #[gpui_kit::test]
    fn clicking_an_add_action_keeps_the_drawer_out(cx: &mut TestAppContext) {
        let (view, _, _, cx) = setup(cx);
        reading(&view, ReadMode::Stream, cx);
        view.update_in(cx, |v, window, cx| v.stream_move(1, window, cx));
        settle(cx);
        view.update_in(cx, |v, window, cx| v.open_notes(window, cx));
        settle(cx);
        let shown = view.read_with(cx, |v, _| v.notes.shown);
        let chip = cx.debug_bounds("stream-act-Notes").expect("chip");
        cx.simulate_click(chip.center(), gpui_kit::Modifiers::none());
        settle(cx);
        view.read_with(cx, |v, cx| {
            assert!(v.notes.open && !v.notes.closing, "stayed out");
            assert_eq!(v.notes.shown, shown, "no new slide-in");
            assert!(v.notes.text(cx).contains("![["), "added");
        });
        // A click on the stream, away from any action: closes it.
        let row = cx.debug_bounds("stream-actions").expect("row");
        cx.simulate_click(
            gpui_kit::point(row.left() + px(4.), row.top() - px(30.)),
            gpui_kit::Modifiers::none(),
        );
        settle(cx);
        assert!(!is_open(&view, cx), "closed by a click outside");
    }

    #[gpui_kit::test]
    fn the_drawer_leaves_esc_to_the_other_panes(cx: &mut TestAppContext) {
        let (view, _, _, cx) = setup(cx);
        reading(&view, ReadMode::Stream, cx);
        let key = key_of(&view, LIN_GARDENS, cx);
        view.update_in(cx, |v, window, cx| v.open_reading(key, window, cx));
        settle(cx);
        assert!(view.read_with(cx, |v, _| v.reading.opened.is_some()));
        // Open: esc is the drawer's; the side pane stays.
        cx.simulate_keystrokes(&crate::keymap::keys("cmd-shift-n"));
        settle(cx);
        cx.simulate_keystrokes("escape");
        settle(cx);
        assert!(!is_open(&view, cx));
        assert!(
            view.read_with(cx, |v, _| v.reading.opened.is_some()),
            "only the drawer closed"
        );
        slide_out(cx);
        // Closed: esc goes to the stream, which closes its side pane.
        cx.simulate_keystrokes("escape");
        settle(cx);
        view.read_with(cx, |v, _| {
            assert!(v.reading.opened.is_none(), "the pane got esc");
            assert_eq!(v.reading.view, View::Reading);
        });
        // And the browser pane keeps its esc too.
        view.update_in(cx, |v, window, cx| {
            v.open_url_in_app(
                "https://blyg.example.com/",
                crate::app::browser::OpenMode::Slide,
                window,
                cx,
            );
        });
        settle(cx);
        cx.simulate_keystrokes("escape");
        settle(cx);
        view.read_with(cx, |v, _| assert!(!v.browser.open));
    }

    // ------------------------------------------------ universal quoting

    fn browse(view: &Entity<MainView>, url: &str, cx: &mut VisualTestContext) {
        view.update_in(cx, |v, window, cx| {
            v.open_url_in_app(url, crate::app::browser::OpenMode::Slide, window, cx);
        });
        settle(cx);
        view.update_in(cx, |v, _, cx| v.browser_refresh_state(cx));
    }

    fn quote(view: &Entity<MainView>, cx: &mut VisualTestContext) {
        view.update_in(cx, |v, window, cx| {
            v.quote_to_draft(&crate::app::notes::QuoteToDraft, window, cx)
        });
        settle(cx);
    }

    fn editor_text(view: &Entity<MainView>, cx: &mut VisualTestContext) -> String {
        view.read_with(cx, |v, cx| v.editor.read(cx).value().to_string())
    }

    /// A draft being written, with the caret at `caret`.
    fn writing(
        view: &Entity<MainView>,
        fake: &FakeBackend,
        kind: Kind,
        text: &str,
        caret: usize,
        cx: &mut VisualTestContext,
    ) -> blyg_core::LocalId {
        let id = fake.create_draft(kind, text).unwrap();
        view.update_in(cx, |v, window, cx| {
            v.open(&id, window, cx);
            v.editor
                .update(cx, |s, cx| s.set_selected_range(caret..caret, cx));
        });
        settle(cx);
        id
    }

    /// A passage on a web page, with no draft open: a new fragment holding
    /// the quote and the page's link. With nothing selected: nothing made.
    #[gpui_kit::test]
    fn a_web_passage_starts_a_draft_with_its_link(cx: &mut TestAppContext) {
        let (view, fake, log, cx) = setup(cx);
        view.update(cx, |v, _| v.current = None);
        browse(&view, "https://example.com/tides", cx);
        let before = fake.items().len();
        quote(&view, cx);
        assert_eq!(fake.items().len(), before, "nothing selected, nothing made");
        log.borrow_mut().selection = "High water\n\nat 6:12".into();
        quote(&view, cx);
        let want = "> High water\n>\n> at 6:12\n>\n> — [Tide tables \\[2026\\]](https://example.com/tides)\n\n";
        let made = fake
            .items()
            .into_iter()
            .find(|i| i.content_md == want)
            .expect("a new draft with the quote");
        assert_eq!(made.kind, Kind::Fragment);
        assert_eq!(made.status, Status::Draft);
        assert!(made.stub_of.is_none());
        assert_eq!(editor_text(&view, cx), want);
        assert!(
            !view.read_with(cx, |v, _| v.browser.open),
            "back to the draft"
        );
    }

    /// A passage on a held blyg post's page, into the thread being written:
    /// the partial transclusion, at the caret.
    #[gpui_kit::test]
    fn a_blyg_passage_goes_in_the_thread_at_the_caret(cx: &mut TestAppContext) {
        let (view, fake, log, cx) = setup(cx);
        let id = writing(&view, &fake, Kind::Thread, "Before.\n\nAfter.", 7, cx);
        browse(&view, &format!("{LIN}t/{LIN_GARDENS}/"), cx);
        log.borrow_mut().selection = "Gardens, not streams".into();
        quote(&view, cx);
        let want = format!("Before.\n\n![[{LIN_GARDENS}]]\n> Gardens, not streams\n\nAfter.");
        assert_eq!(editor_text(&view, cx), want);
        assert_eq!(fake.item(&id).unwrap().content_md, want, "saved");
        // Into a fragment the same passage can't transclude: the plain form.
        writing(&view, &fake, Kind::Fragment, "", 0, cx);
        browse(&view, &format!("{LIN}t/{LIN_GARDENS}/"), cx);
        quote(&view, cx);
        let text = editor_text(&view, cx);
        assert!(
            text.starts_with("> Gardens, not streams\n>\n> — [Tide tables")
                && !text.contains("![["),
            "{text}"
        );
    }

    /// The reading pane's post: its passage, partial into a new thread.
    #[gpui_kit::test]
    fn a_reader_passage_starts_a_thread(cx: &mut TestAppContext) {
        let (view, fake, log, cx) = setup(cx);
        view.update(cx, |v, _| v.current = None);
        reading(&view, ReadMode::Reader, cx);
        let key = key_of(&view, LIN_GARDENS, cx);
        view.update_in(cx, |v, window, cx| v.open_reading(key, window, cx));
        settle(cx);
        log.borrow_mut().selection = "Gardens, not streams".into();
        quote(&view, cx);
        let want = format!("![[{LIN_GARDENS}]]\n> Gardens, not streams\n\n");
        let made = fake
            .items()
            .into_iter()
            .find(|i| i.content_md == want)
            .expect("a thread with the partial quote");
        assert_eq!(made.kind, Kind::Thread);
        assert!(made.stub_of.is_none(), "a quote, not a reply");
    }

    /// Your notes: a passage from a quote there keeps that quote's link;
    /// your own words go without one.
    #[gpui_kit::test]
    fn a_notes_passage_keeps_the_link_it_was_quoted_with(cx: &mut TestAppContext) {
        let (view, fake, log, cx) = setup(cx);
        let id = writing(&view, &fake, Kind::Thread, "Draft.", 6, cx);
        browse(&view, "https://example.com/tides", cx);
        log.borrow_mut().selection = "High water at 6:12".into();
        view.update_in(cx, |v, window, cx| v.browser_send_to_notes(window, cx));
        settle(cx);
        view.update_in(cx, |v, window, cx| {
            v.close_browser(window, cx);
            v.open_notes(window, cx);
        });
        settle(cx);
        let select = |view: &Entity<MainView>, s: &str, cx: &mut VisualTestContext| {
            view.update_in(cx, |v, _, cx| {
                let editor = v.notes.editor.clone().unwrap();
                let at = editor.read(cx).value().find(s).expect("in the notes");
                editor.update(cx, |e, cx| e.set_selected_range(at..at + s.len(), cx));
            });
        };
        select(&view, "water at", cx);
        quote(&view, cx);
        let text = fake.item(&id).unwrap().content_md;
        assert!(
            text.contains("> water at\n>\n> — [Tide tables \\[2026\\]](https://example.com/tides)"),
            "{text}"
        );
        // Own words in the notes: quoted with no link.
        view.update_in(cx, |v, window, cx| {
            let editor = v.notes.editor.clone().unwrap();
            let end = editor.read(cx).value().len();
            editor.update(cx, |e, cx| {
                e.set_selected_range(end..end, cx);
                e.replace("My own idea.".to_string(), window, cx);
            });
            v.open_notes(window, cx);
        });
        settle(cx);
        select(&view, "own idea", cx);
        quote(&view, cx);
        let text = fake.item(&id).unwrap().content_md;
        assert!(text.contains("\n\n> own idea\n\n"), "{text}");
    }

    // ------------------------------------------------ selecting with the mouse

    fn reader_event(
        view: &Entity<MainView>,
        ev: webview::SurfaceEvent,
        cx: &mut VisualTestContext,
    ) {
        view.update_in(cx, |v, window, cx| v.reader_event(ev, window, cx));
        settle(cx);
    }

    fn open_in_reader(view: &Entity<MainView>, id: &str, cx: &mut VisualTestContext) {
        if view.read_with(cx, |v, _| v.reading.view != View::Reading) {
            reading(view, ReadMode::Reader, cx);
        }
        let key = key_of(view, id, cx);
        view.update_in(cx, |v, window, cx| v.open_reading(key, window, cx));
        settle(cx);
    }

    /// The reader's page gets the pill by a selection once it's loaded
    /// ("Quote in draft"; no "Reply with this" since studio 0.31), for a
    /// blyg post and a feed post alike.
    #[gpui_kit::test]
    fn the_reader_turns_on_the_selection_pill(cx: &mut TestAppContext) {
        let (view, _, log, cx) = setup(cx);
        open_in_reader(&view, LIN_GARDENS, cx);
        reader_event(&view, webview::SurfaceEvent::Ready, cx);
        assert!(
            log.borrow()
                .reader_evals
                .contains(&webview::SELECTION_UI_JS.to_string()),
            "{:?}",
            log.borrow().reader_evals
        );
        let feed = view.read_with(cx, |v, _| {
            v.reading
                .rows
                .iter()
                .find(|r| r.subscription_id == "sub-omar")
                .map(|r| r.remote_id.clone())
                .expect("a feed post")
        });
        open_in_reader(&view, &feed, cx);
        log.borrow_mut().reader_evals.clear();
        reader_event(&view, webview::SurfaceEvent::Ready, cx);
        assert_eq!(
            log.borrow().reader_evals,
            vec![webview::SELECTION_UI_JS.to_string()]
        );
        // Not on your own post (Posts shows it here too).
        view.update_in(cx, |v, window, cx| v.show_view(View::Posts, window, cx));
        settle(cx);
        assert!(!view.read_with(cx, |v, _| v.reader_selection_ui()));
    }

    /// The pill's "Quote in draft": the passage, partial, into a new
    /// thread. An empty selection makes nothing.
    #[gpui_kit::test]
    fn the_pill_quotes_the_selection(cx: &mut TestAppContext) {
        let (view, fake, log, cx) = setup(cx);
        view.update(cx, |v, _| v.current = None);
        open_in_reader(&view, LIN_GARDENS, cx);
        let before = fake.items().len();
        reader_event(&view, webview::SurfaceEvent::QuoteSelection, cx);
        assert_eq!(fake.items().len(), before, "nothing selected, nothing made");
        log.borrow_mut().selection = "Gardens, not streams".into();
        reader_event(&view, webview::SurfaceEvent::QuoteSelection, cx);
        let want = format!("![[{LIN_GARDENS}]]\n> Gardens, not streams\n\n");
        assert!(
            fake.items().iter().any(|i| i.content_md == want),
            "a thread with the partial quote"
        );
    }

    /// The pill quotes the post even while the notes drawer has the
    /// keyboard and a selection of its own (⇧⌘D would take the notes').
    #[gpui_kit::test]
    fn the_pill_quotes_the_post_not_the_notes(cx: &mut TestAppContext) {
        let (view, fake, log, cx) = setup(cx);
        let id = writing(&view, &fake, Kind::Thread, "Draft.", 6, cx);
        open_in_reader(&view, LIN_GARDENS, cx);
        view.update_in(cx, |v, window, cx| {
            v.open_notes(window, cx);
            let editor = v.notes.editor.clone().unwrap();
            editor.update(cx, |e, cx| {
                e.replace("A thought of mine.".to_string(), window, cx);
                e.set_selected_range(2..9, cx);
            });
        });
        settle(cx);
        log.borrow_mut().selection = "Gardens, not streams".into();
        reader_event(&view, webview::SurfaceEvent::QuoteSelection, cx);
        let text = fake.item(&id).unwrap().content_md;
        assert!(text.contains("> Gardens, not streams"), "{text}");
        assert!(!text.contains("thought"), "{text}");
    }

    /// Reply quotes the whole post, a selection in the page or not (studio
    /// 0.31: the passage is chosen in the stub editor).
    #[gpui_kit::test]
    fn reply_quotes_the_whole_post(cx: &mut TestAppContext) {
        let (view, fake, log, cx) = setup(cx);
        open_in_reader(&view, LIN_GARDENS, cx);
        log.borrow_mut().selection = "Gardens, not streams".into();
        view.update_in(cx, |v, window, cx| {
            v.reading_action_for_test("Reply", window, cx)
        });
        settle(cx);
        let stub = fake
            .items()
            .into_iter()
            .find(|i| i.stub_of.as_ref().is_some_and(|s| s.id == LIN_GARDENS))
            .expect("a stub");
        assert_eq!(stub.content_md, format!("![[{LIN_GARDENS}]]\n\n"));
    }

    /// ⌘C with the keyboard on the list copies the post's selection (the
    /// page says it has one); with none, the clipboard is left alone.
    #[gpui_kit::test]
    fn copy_takes_the_posts_selection(cx: &mut TestAppContext) {
        use gpui_kit::ClipboardItem;
        use gpui_kit::base::input::Copy;
        let (view, _, log, cx) = setup(cx);
        open_in_reader(&view, LIN_GARDENS, cx);
        cx.write_to_clipboard(ClipboardItem::new_string("before".into()));
        log.borrow_mut().selection = "Gardens, not streams".into();
        cx.dispatch_action(Copy);
        settle(cx);
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
            Some("before"),
            "the page didn't say it has a selection"
        );
        reader_event(&view, webview::SurfaceEvent::Selected(true), cx);
        cx.dispatch_action(Copy);
        settle(cx);
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
            Some("Gardens, not streams")
        );
    }

    /// "→ Draft" in the browser keeps the page's keyboard, so its
    /// selection is still there when the click reads it; any other click
    /// on the chrome gives the keyboard back (and WebKit drops it).
    #[gpui_kit::test]
    fn the_draft_button_keeps_the_pages_selection(cx: &mut TestAppContext) {
        let (view, fake, log, cx) = setup(cx);
        let id = writing(&view, &fake, Kind::Fragment, "", 0, cx);
        browse(&view, "https://example.com/tides", cx);
        // The pane has slid in.
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(300));
        settle(cx);
        {
            let mut l = log.borrow_mut();
            l.page_keyboard = true;
            l.selection = "High water at 6:12".into();
        }
        let back = crate::app::toolbar::tests::settled_bounds(cx, "browser-back");
        cx.simulate_click(back.center(), gpui_kit::Modifiers::none());
        settle(cx);
        assert!(log.borrow().selection.is_empty(), "a chrome click drops it");
        {
            let mut l = log.borrow_mut();
            l.page_keyboard = true;
            l.selection = "High water at 6:12".into();
        }
        let draft = crate::app::toolbar::tests::settled_bounds(cx, "browser-draft");
        cx.simulate_click(draft.center(), gpui_kit::Modifiers::none());
        settle(cx);
        let text = fake.item(&id).unwrap().content_md;
        assert!(
            text.starts_with("> High water at 6:12\n>\n> — [Tide tables"),
            "{text}"
        );
    }

    /// "→ Draft" says when there's a selection to quote (the pane looks
    /// while the page has the keyboard).
    #[gpui_kit::test]
    fn the_draft_button_notices_a_selection(cx: &mut TestAppContext) {
        let (view, _, log, cx) = setup(cx);
        browse(&view, "https://example.com/tides", cx);
        let selected = |view: &Entity<MainView>, cx: &mut VisualTestContext| {
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(450));
            settle(cx);
            view.read_with(cx, |v, _| v.browser.selected)
        };
        log.borrow_mut().selection = "High water".into();
        assert!(!selected(&view, cx), "the page doesn't have the keyboard");
        log.borrow_mut().page_keyboard = true;
        assert!(selected(&view, cx));
        log.borrow_mut().selection.clear();
        assert!(!selected(&view, cx));
    }

    /// The drawer's footer says how to quote a passage selected in the
    /// notes, and a click on it does.
    #[gpui_kit::test]
    fn the_notes_footer_offers_to_quote_the_selection(cx: &mut TestAppContext) {
        let (view, fake, _, cx) = setup(cx);
        let id = writing(&view, &fake, Kind::Thread, "Draft.", 6, cx);
        view.update_in(cx, |v, window, cx| {
            v.open_notes(window, cx);
            let editor = v.notes.editor.clone().unwrap();
            editor.update(cx, |e, cx| {
                e.replace("An idea to keep.".to_string(), window, cx)
            });
        });
        settle(cx);
        assert!(
            cx.debug_bounds("notes-quote-selection").is_none(),
            "nothing selected"
        );
        view.update_in(cx, |v, _, cx| {
            let editor = v.notes.editor.clone().unwrap();
            let at = editor.read(cx).value().find("idea").unwrap();
            editor.update(cx, |e, cx| e.set_selected_range(at..at + 4, cx));
            cx.notify();
        });
        settle(cx);
        let hint = cx
            .debug_bounds("notes-quote-selection")
            .expect("the footer's hint");
        cx.simulate_click(hint.center(), gpui_kit::Modifiers::none());
        settle(cx);
        let text = fake.item(&id).unwrap().content_md;
        assert!(text.contains("> idea"), "{text}");
    }

    /// A drag over a post in the stream (its text can't be selected there)
    /// opens it in the side pane, where it can; so does a double-click.
    #[gpui_kit::test]
    fn a_drag_in_the_stream_opens_the_post_to_select_in(cx: &mut TestAppContext) {
        use crate::app::reading::stream_vm::PostClick;
        let (view, _, _, cx) = setup(cx);
        reading(&view, ReadMode::Stream, cx);
        let key = key_of(&view, LIN_GARDENS, cx);
        let opened = |view: &Entity<MainView>, cx: &mut VisualTestContext| {
            view.read_with(cx, |v, _| v.reading.opened.as_ref().map(|o| o.key.clone()))
        };
        let click = |view: &Entity<MainView>, c: PostClick, cx: &mut VisualTestContext| {
            let k = key.clone();
            view.update_in(cx, |v, window, cx| v.stream_post_click(k, c, window, cx));
            settle(cx);
        };
        click(&view, PostClick::Select, cx);
        assert_eq!(opened(&view, cx), None, "a click selects");
        click(&view, PostClick::OpenToSelect, cx);
        assert_eq!(opened(&view, cx), Some(key.clone()));
        view.update_in(cx, |v, window, cx| v.close_stream_pane(window, cx));
        settle(cx);
        click(&view, PostClick::Open, cx);
        assert_eq!(opened(&view, cx), Some(key.clone()));
    }
}
