//! `BLYGGER_DEMO=cm-*`: the composer's scripted walkthroughs, for snapshots.
//! - `cm-mention`: typing `@a` in a thread shows the mention popup.
//! - `cm-spell`: misspellings underlined (the system spell checker).
//! - `cm-spell-menu`: the spelling menu on a flagged word.
//! - `cm-link`: typing `[[gar` shows the link picker, typed in the editor.
//! - `cm-quote`: typing `![[` on its own line in a thread: the quote picker.

use blyg_core::LocalId;
use gpui_kit::*;

use super::MainView;

const DEMO_POST: &str = "01J9H4C";
const TYPO_LINE: &str =
    "\n\nThe tide tabels are a promiss the sea never signd, and https://tides.example.org agrees.";

impl MainView {
    pub(super) fn composer_demo(
        &mut self,
        scenario: &str,
        n: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match (scenario, n) {
            ("cm-mention", 0) => {
                self.composer_demo_open(window, cx);
                self.demo_type("\n\nWith thanks to ", window, cx);
            }
            // One keystroke per step: the popup reacts to each edit event.
            ("cm-mention", 1) => self.demo_type("@", window, cx),
            ("cm-mention", 2) => self.demo_type("a", window, cx),
            ("cm-link" | "cm-quote", 0) => {
                self.composer_demo_open(window, cx);
                let lead = if scenario == "cm-link" {
                    "\n\nSee also ["
                } else {
                    "\n\n!["
                };
                self.demo_type(lead, window, cx);
            }
            // The second bracket is its own keystroke: that opens the picker.
            // (A plain insert, as a keystroke: the edit event does the rest.)
            ("cm-link" | "cm-quote", 1) => self
                .editor
                .update(cx, |s, cx| s.insert("[".to_string(), window, cx)),
            ("cm-link", 2) => self
                .editor
                .update(cx, |s, cx| s.insert("gar".to_string(), window, cx)),
            ("cm-spell" | "cm-spell-menu", 0) => {
                self.composer_demo_open(window, cx);
                self.demo_type(TYPO_LINE, window, cx);
            }
            ("cm-spell-menu", 1) => {
                let text = self.editor.read(cx).value().to_string();
                if let Some(at) = text.rfind("tabels") {
                    self.assist
                        .update(cx, |a, cx| a.open_spell_menu_at(at + 2, window, cx));
                }
            }
            _ => {}
        }
        if n == 2 {
            super::reading::demo::snapshot_later(window, cx);
        }
    }

    fn composer_demo_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open(&LocalId(DEMO_POST.into()), window, cx);
    }
}
