//! Headless GPUI tests of the composer in the main editor: the @-mention
//! popup and spellcheck, through real keystrokes on a zero-latency
//! FakeBackend and a fake spell checker.

use std::sync::Arc;
use std::time::Duration;

use blyg_core::config::MemoryTokenStore;
use blyg_core::{Backend, ConfigStore, LocalId};
use gpui_kit::{ClipboardItem, Entity, TestAppContext, VisualTestContext};

use super::{MainView, Mode};
use crate::composer::spell::fake::FakeSpell;
use crate::composer::{self, AssistKey};
use crate::fake::reading_seed::*;
use crate::fake::{FakeBackend, Timing};
use crate::prefs::Prefs;

const CONNECTED: &str = "# test config\nblyg-url = https://blyg.example.com\n";
const DRAFT_THREAD: &str = "01J9H4C";

fn setup(cx: &mut TestAppContext) -> (Entity<MainView>, Arc<FakeSpell>, &mut VisualTestContext) {
    let prefs = Prefs::from_config(ConfigStore::in_memory(CONNECTED).config());
    let spell = Arc::new(FakeSpell::new(&[
        ("teh", &["the", "tea"]),
        ("wrold", &["world"]),
    ]));
    let engine = spell.clone();
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::app::bind_keys(cx);
        crate::settings::init(
            ConfigStore::in_memory(CONNECTED),
            Arc::new(MemoryTokenStore::default()),
            None,
            cx,
        );
        composer::init(Some(engine), true, cx);
    });
    let fake = Arc::new(FakeBackend::with_timing(Timing::instant()).without_media_cache());
    let backend: Arc<dyn Backend> = fake.clone();
    let (view, cx) = cx.add_window_view(move |window, cx| {
        MainView::new(
            backend,
            Some(fake),
            prefs,
            std::time::Instant::now(),
            window,
            cx,
        )
    });
    cx.run_until_parked();
    (view, spell, cx)
}

/// Open a post with the caret at the end of the editor, on a fresh line.
fn open_at_end(view: &Entity<MainView>, id: &str, cx: &mut VisualTestContext) -> String {
    view.update_in(cx, |v, window, cx| {
        v.open(&LocalId(id.into()), window, cx);
        v.editor.update(cx, |s, cx| {
            s.focus(window, cx);
            let end = s.text().len();
            s.set_selected_range(end..end, cx);
        });
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    text(view, cx)
}

fn text(view: &Entity<MainView>, cx: &mut VisualTestContext) -> String {
    view.read_with(cx, |v, cx| v.editor.read(cx).value().to_string())
}

fn labels(view: &Entity<MainView>, cx: &mut VisualTestContext) -> Option<Vec<String>> {
    view.read_with(cx, |v, cx| v.assist.read(cx).mention_labels())
}

fn misspelled(view: &Entity<MainView>, cx: &mut VisualTestContext) -> Vec<String> {
    view.read_with(cx, |v, cx| {
        let t = v.editor.read(cx).value().to_string();
        v.assist
            .read(cx)
            .misspelled()
            .iter()
            .map(|r| t[r.clone()].to_string())
            .collect()
    })
}

/// Let the ~300 ms typing pause pass, and the check land.
fn pause(cx: &mut VisualTestContext) {
    cx.executor()
        .advance_clock(composer::assist::SPELL_DEBOUNCE + Duration::from_millis(10));
    cx.run_until_parked();
}

// ------------------------------------------------------------ mentions

#[gpui_kit::test]
fn typing_at_offers_known_blygs_and_enter_inserts_a_link(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    let before = open_at_end(&view, DRAFT_THREAD, cx);
    cx.simulate_input("Thanks ");
    cx.run_until_parked();
    assert_eq!(labels(&view, cx), None);
    cx.simulate_input("@");
    cx.run_until_parked();
    let all = labels(&view, cx).expect("`@` opens the popup");
    for name in ["Ada", "Rue", "Lin", "Omar"] {
        assert!(all.iter().any(|l| l == name), "{name} in {all:?}");
    }
    cx.simulate_input("ad");
    cx.run_until_parked();
    assert_eq!(labels(&view, cx).unwrap(), ["Ada"], "filtered as you type");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(labels(&view, cx), None, "picking closes it");
    assert_eq!(
        text(&view, cx),
        format!("{before}Thanks [Ada]({ADA})"),
        "a plain link to the blyg, and no new line"
    );
}

#[gpui_kit::test]
fn arrows_choose_and_tab_picks_too(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    let before = open_at_end(&view, DRAFT_THREAD, cx);
    cx.simulate_input("@");
    cx.run_until_parked();
    let all = labels(&view, cx).unwrap();
    cx.simulate_keystrokes("down down up down");
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    let second = &all[2];
    let t = text(&view, cx);
    assert!(
        t.starts_with(&format!("{before}[{second}](")),
        "↓↓↑↓ lands on the third: {t}"
    );
    view.read_with(cx, |v, _| assert_eq!(v.mode, Mode::Edit));
}

#[gpui_kit::test]
fn esc_keeps_what_was_typed_and_stays_in_the_editor(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    let before = open_at_end(&view, DRAFT_THREAD, cx);
    cx.simulate_input("@ru");
    cx.run_until_parked();
    assert!(labels(&view, cx).is_some());
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(labels(&view, cx), None);
    assert_eq!(text(&view, cx), format!("{before}@ru"));
    view.read_with(cx, |v, _| {
        assert_eq!(v.mode, Mode::Edit, "esc closed the popup, not the editor")
    });
    // Typing on is literal: it doesn't reopen.
    cx.simulate_input("e");
    cx.run_until_parked();
    assert_eq!(labels(&view, cx), None);
    assert_eq!(text(&view, cx), format!("{before}@rue"));
}

#[gpui_kit::test]
fn no_match_lets_enter_be_a_new_line(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    let before = open_at_end(&view, DRAFT_THREAD, cx);
    cx.simulate_input("@zzz");
    cx.run_until_parked();
    assert_eq!(labels(&view, cx), Some(vec![]));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(labels(&view, cx), None);
    assert_eq!(text(&view, cx), format!("{before}@zzz\n"));
}

#[gpui_kit::test]
fn emails_pastes_and_code_never_open_it(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    let before = open_at_end(&view, DRAFT_THREAD, cx);
    cx.simulate_input("ada@");
    cx.run_until_parked();
    assert_eq!(labels(&view, cx), None, "an email address");
    cx.simulate_input(" ");
    cx.write_to_clipboard(ClipboardItem::new_string("@".into()));
    cx.simulate_keystrokes(&crate::keymap::keys("cmd-v"));
    cx.run_until_parked();
    assert_eq!(labels(&view, cx), None, "a paste");
    assert_eq!(text(&view, cx), format!("{before}ada@ @"));
    cx.simulate_keystrokes("enter");
    cx.simulate_input("```");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("@");
    cx.run_until_parked();
    assert_eq!(labels(&view, cx), None, "inside a code fence");
}

// ------------------------------------------------------------ spelling

#[gpui_kit::test]
fn misspellings_are_flagged_after_a_pause_and_not_while_typing(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    open_at_end(&view, DRAFT_THREAD, cx);
    cx.simulate_input("Hello teh wrold ");
    cx.run_until_parked();
    assert!(
        misspelled(&view, cx).is_empty(),
        "nothing until typing pauses"
    );
    pause(cx);
    assert_eq!(misspelled(&view, cx), ["teh", "wrold"]);
    // Typing on at the end of a flagged word hides its underline.
    view.update_in(cx, |v, window, cx| {
        let t = v.editor.read(cx).value().to_string();
        let end = t.rfind("wrold").unwrap() + 5;
        v.editor.update(cx, |s, cx| {
            s.focus(window, cx);
            s.set_selected_range(end..end, cx);
        });
    });
    cx.simulate_input("s");
    cx.run_until_parked();
    assert_eq!(misspelled(&view, cx), ["teh"], "the others stay put");
}

#[gpui_kit::test]
fn code_links_and_markup_are_never_checked(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    open_at_end(&view, DRAFT_THREAD, cx);
    cx.simulate_input("`teh` [teh](https://teh.example.com/wrold) teh.example.org [TK]teh[/TK] ");
    pause(cx);
    assert_eq!(
        misspelled(&view, cx),
        ["teh", "teh"],
        "only the link text and the TK instruction"
    );
}

#[gpui_kit::test]
fn only_changed_lines_are_rechecked(cx: &mut TestAppContext) {
    let (view, spell, cx) = setup(cx);
    open_at_end(&view, DRAFT_THREAD, cx);
    pause(cx);
    let after_open = spell.checked();
    cx.simulate_input("teh");
    pause(cx);
    assert_eq!(
        spell.checked(),
        after_open + 1,
        "one line changed, one line checked"
    );
    assert!(
        misspelled(&view, cx).is_empty(),
        "the word at the caret isn't finished, so it isn't flagged yet"
    );
    cx.simulate_input(" ok");
    pause(cx);
    assert_eq!(spell.checked(), after_open + 2);
    assert_eq!(misspelled(&view, cx), ["teh"]);
}

#[gpui_kit::test]
fn the_spelling_menu_replaces_learns_and_ignores(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    let before = open_at_end(&view, DRAFT_THREAD, cx);
    cx.simulate_input("teh wrold teh ");
    pause(cx);
    assert_eq!(misspelled(&view, cx), ["teh", "wrold", "teh"]);
    let at = before.len();
    let menu = view.update_in(cx, |v, window, cx| {
        v.assist.update(cx, |a, cx| {
            a.open_spell_menu_at(at + 1, window, cx);
            a.menu_labels()
        })
    });
    assert_eq!(menu.unwrap(), ["the", "tea", "Learn Spelling", "Ignore"]);
    cx.run_until_parked();
    assert_eq!(
        misspelled(&view, cx),
        ["teh", "wrold", "teh"],
        "still underlined under the menu"
    );
    cx.simulate_keystrokes("down enter");
    cx.run_until_parked();
    assert_eq!(
        text(&view, cx),
        format!("{before}tea wrold teh "),
        "the second guess"
    );
    assert_eq!(misspelled(&view, cx), ["wrold", "teh"]);

    // Learn Spelling: every "wrold" stops being flagged.
    let wrold = text(&view, cx).find("wrold").unwrap();
    view.update_in(cx, |v, window, cx| {
        v.assist.update(cx, |a, cx| {
            a.open_spell_menu_at(wrold, window, cx);
            a.pick_menu(1, window, cx); // [world, Learn Spelling, Ignore]
        })
    });
    pause(cx);
    assert_eq!(misspelled(&view, cx), ["teh"]);

    // Ignore, from the keyboard: esc first just closes the menu.
    let teh = text(&view, cx).rfind("teh").unwrap();
    view.update_in(cx, |v, window, cx| {
        v.assist
            .update(cx, |a, cx| a.open_spell_menu_at(teh + 3, window, cx))
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    view.read_with(cx, |v, cx| {
        assert_eq!(v.assist.read(cx).menu_labels(), None);
        assert_eq!(v.mode, Mode::Edit);
    });
    view.update_in(cx, |v, window, cx| {
        v.assist.update(cx, |a, cx| {
            a.open_spell_menu_at(teh, window, cx);
            let ignore = a.menu_labels().unwrap().len() - 1;
            a.pick_menu(ignore, window, cx);
        })
    });
    pause(cx);
    assert!(misspelled(&view, cx).is_empty());
}

#[gpui_kit::test]
fn a_click_on_a_correct_word_opens_no_menu(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    let before = open_at_end(&view, DRAFT_THREAD, cx);
    cx.simulate_input("fine words ");
    pause(cx);
    let menu = view.update_in(cx, |v, window, cx| {
        v.assist.update(cx, |a, cx| {
            a.open_spell_menu_at(before.len() + 1, window, cx);
            a.menu_labels()
        })
    });
    assert_eq!(menu, None);
    let took = view.update_in(cx, |v, window, cx| {
        v.assist
            .update(cx, |a, cx| a.handle_key(AssistKey::Enter, window, cx))
    });
    assert!(!took);
}

#[gpui_kit::test]
fn turning_spellcheck_off_clears_the_underlines(cx: &mut TestAppContext) {
    let (view, spell, cx) = setup(cx);
    open_at_end(&view, DRAFT_THREAD, cx);
    cx.simulate_input("teh ");
    pause(cx);
    assert_eq!(misspelled(&view, cx), ["teh"]);
    cx.update(|_, cx| composer::set_spellcheck(false, cx));
    cx.run_until_parked();
    assert!(misspelled(&view, cx).is_empty());
    let n = spell.checked();
    cx.simulate_input("wrold ");
    pause(cx);
    assert_eq!(spell.checked(), n, "off means no checking at all");
    cx.update(|_, cx| composer::set_spellcheck(true, cx));
    pause(cx);
    assert_eq!(misspelled(&view, cx), ["teh", "wrold"]);
}

// ------------------------------------------------------------ speed

/// Typing latency with spellcheck on vs off, on a ~60 KB post (run with
/// `cargo test -p blyg-app --release typing_latency -- --ignored --nocapture`).
/// Each keystroke goes through the whole app: the editor, the save, the
/// composer's edit hook. The check itself runs later, off the typing path.
#[gpui_kit::test]
#[ignore]
fn typing_latency_with_and_without_spellcheck(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    open_at_end(&view, DRAFT_THREAD, cx);
    let big = LATENCY_PARA.repeat(560);
    let mut per_key = Vec::new();
    for on in [false, true, false, true] {
        cx.update(|_, cx| composer::set_spellcheck(on, cx));
        view.update_in(cx, |v, window, cx| {
            v.set_editor_text(&big, window, cx);
            v.editor.update(cx, |s, cx| {
                s.focus(window, cx);
                let end = s.text().len();
                s.set_selected_range(end..end, cx);
            });
        });
        pause(cx);
        let keys = 300;
        let t0 = std::time::Instant::now();
        for _ in 0..keys / 6 {
            cx.simulate_input("words ");
            cx.run_until_parked();
        }
        let us = t0.elapsed().as_micros() as f64 / keys as f64;
        per_key.push((on, us));
        pause(cx);
    }
    for (on, us) in &per_key {
        println!(
            "typing_latency doc={} KB spellcheck={on}: {us:.0} µs/keystroke",
            big.len() / 1024
        );
    }
}

const LATENCY_PARA: &str = "Tide tables are a kind of promise teh sea never signed, and [a link](https://tides.example.org/x) `code` too.\n\n";

/// The composer's own per-keystroke work, and the debounced plan, on the
/// same ~60 KB post.
#[test]
#[ignore]
fn composer_hook_costs() {
    use crate::composer::spell;
    let old = LATENCY_PARA.repeat(560);
    let mut new = old.clone();
    new.insert(old.len() / 2, 'x');
    let n = 200;
    let t0 = std::time::Instant::now();
    for _ in 0..n {
        let (edit, ins) = crate::vm::splice(&old, &new);
        let mut r: Vec<_> = (0..500).map(|i| i * 100..i * 100 + 3).collect();
        spell::shift(&mut r, &edit, ins.len());
        std::hint::black_box(r);
    }
    let edit_us = t0.elapsed().as_micros() as f64 / n as f64;
    let mut cache = spell::Cache::default();
    let masked = spell::mask(&old);
    for l in cache.plan(&masked).missing {
        cache.insert(l, vec![]);
    }
    let t0 = std::time::Instant::now();
    for _ in 0..n {
        let m = spell::mask(&new);
        std::hint::black_box(cache.plan(&m));
    }
    let plan_us = t0.elapsed().as_micros() as f64 / n as f64;
    println!(
        "composer_hook doc={} KB: per-keystroke splice+shift {edit_us:.0} µs; debounced mask+plan {plan_us:.0} µs",
        old.len() / 1024
    );
}

/// Through the editor's own right-click, not `open_spell_menu_at`: its
/// callback runs inside the editor's update, which once crashed the app.
#[gpui_kit::test]
fn a_real_right_click_opens_the_spelling_menu(cx: &mut TestAppContext) {
    let (view, _spell, cx) = setup(cx);
    open_at_end(&view, DRAFT_THREAD, cx);
    cx.simulate_input("I saw teh sea");
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    assert_eq!(misspelled(&view, cx), vec!["teh".to_string()]);
    let at = view.read_with(cx, |v, cx| {
        let s = v.editor.read(cx);
        let off = s.value().find("teh").unwrap() + 1;
        s.range_to_bounds(&(off..off)).unwrap().center()
    });
    cx.simulate_mouse_down(
        at,
        gpui_kit::MouseButton::Right,
        gpui_kit::Modifiers::default(),
    );
    cx.simulate_mouse_up(
        at,
        gpui_kit::MouseButton::Right,
        gpui_kit::Modifiers::default(),
    );
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, cx| v.assist.read(cx).menu_open()));
}
