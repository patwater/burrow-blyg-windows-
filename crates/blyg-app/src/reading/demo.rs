//! `BLYGGER_DEMO=rd-…`: scripted walkthroughs of the reading screens for
//! screenshots (fake mode), calling the same methods the keys do.

use blyg_core::LocalId;
use gpui_kit::*;

use super::View;
use crate::app::MainView;
use crate::fake::reading_seed::{LIN_GARDENS, RUE_TRUST};

impl MainView {
    pub(crate) fn reading_demo(
        &mut self,
        scenario: &str,
        n: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let open = |this: &mut Self, id: &str, window: &mut Window, cx: &mut Context<Self>| {
            if let Some(k) = this
                .reading
                .rows
                .iter()
                .find(|r| r.remote_id == id)
                .map(super::vm::key)
            {
                this.open_reading(k, window, cx);
            }
        };
        match (scenario, n) {
            // The list with an edited post open: notes + pinned diff.
            ("rd-reading", 0) => self.show_view(View::Reading, window, cx),
            ("rd-reading", 1) => open(self, RUE_TRUST, window, cx),
            // The reading search, with the first match open (#6).
            ("rd-search", 0) => self.show_view(View::Reading, window, cx),
            ("rd-search", 1) => {
                self.focus_reading_search(window, cx);
                self.set_reading_query("garden", window, cx);
                self.move_reading(1, window, cx);
            }
            // A search that matches nothing.
            ("rd-nomatch", 0) => self.show_view(View::Reading, window, cx),
            ("rd-nomatch", 1) => {
                self.focus_reading_search(window, cx);
                self.set_reading_query("zeppelin", window, cx);
            }
            ("rd-notes", 0) => self.show_view(View::Reading, window, cx),
            ("rd-notes", 1) => open(self, LIN_GARDENS, window, cx),
            // Someone else's post on a pinned version, dropdown open.
            ("rd-pill", 0) => self.show_view(View::Reading, window, cx),
            ("rd-pill", 1) => open(self, RUE_TRUST, window, cx),
            ("rd-pill", 2) => {
                self.step_version(-1, cx);
                if let Some(o) = self.reading.opened.as_mut() {
                    o.dropdown = true;
                }
            }
            // Your own post's history, then the pin sheet.
            ("rd-versions", 0) => {
                self.open(&LocalId("01J9M2A".into()), window, cx);
                self.toggle_versions(window, cx);
            }
            ("rd-pin", 0) => {
                self.open(&LocalId("01J9M2A".into()), window, cx);
                self.toggle_versions(window, cx);
            }
            ("rd-pin", 2) => self.ask_pin(window, cx),
            ("rd-mentions", 0) => self.show_view(View::Mentions, window, cx),
            ("rd-subs", 0) => self.show_view(View::Subscriptions, window, cx),
            ("rd-site", 0) => self.open_site_settings(window, cx),
            ("rd-quote", 0) => self.open(&LocalId("01J9H4C".into()), window, cx),
            ("rd-quote", 1) => self.open_quote_picker(window, cx),
            _ => {}
        }
        if n == 2 {
            snapshot_later(window, cx);
        }
    }
}

/// `BLYGGER_SNAPSHOT=<file.png>`: once the demo has played, render the
/// window's last frame to a PNG (Metal, no screen capture, no permissions)
/// and quit. Only in a build made with
/// `RUSTFLAGS="--cfg blygger_snap" cargo build -p blyg-app --features gpui-kit/test-support`
/// (GPUI's `render_to_image` is a test-support API); a no-op otherwise.
pub(crate) fn snapshot_later(window: &mut Window, cx: &mut Context<MainView>) {
    let Ok(path) = std::env::var("BLYGGER_SNAPSHOT") else {
        return;
    };
    cx.spawn_in(window, async move |_, cx| {
        cx.background_executor()
            .timer(std::time::Duration::from_millis(900))
            .await;
        // A window that isn't frontmost may not be redrawing: draw now (which
        // starts any sheet animation), wait it out, and draw again.
        for _ in 0..2 {
            let _ = cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.background_executor()
                .timer(std::time::Duration::from_millis(400))
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

// `blygger_snap` is a local --cfg for screenshot builds only.
#[allow(unexpected_cfgs)]
pub(crate) mod snap {
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
        eprintln!(
            "BLYGGER_SNAPSHOT={path}: this build can't render snapshots (see reading/demo.rs)"
        );
    }
}
