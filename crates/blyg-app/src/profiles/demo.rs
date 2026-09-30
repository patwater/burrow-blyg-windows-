//! `BLYGGER_DEMO=pf-…`: scripted walkthroughs of profiles for screenshots
//! (fake mode), calling the same methods the keys and clicks do.
//!
//! - `pf-reading`: Lin's thread open (the lineage line), then ⌘I.
//! - `pf-blogroll`: Lin's profile, then a blogroll entry's profile (back arrow).
//! - `pf-connections`: Lin's profile on the Connections tab.
//! - `pf-posts`: Rue's profile on the Posts tab.
//! - `pf-feed`: a plain RSS feed's card.
//! - `pf-own`: your own profile, with the blogroll toggles.
//! - `pf-ask`: ⇧⌘O "Open profile…".

use gpui_kit::*;

use super::vm::Tab;
use crate::app::MainView;
use crate::app::reading::View;
use crate::fake::profile_seed::TIDES;
use crate::fake::reading_seed::{LIN, LIN_GARDENS, OMAR, RUE};

impl MainView {
    pub(crate) fn profile_demo(
        &mut self,
        scenario: &str,
        n: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let open_lin = |this: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
            this.show_view(View::Reading, window, cx);
            if let Some(k) = this
                .reading
                .rows
                .iter()
                .find(|r| r.remote_id == LIN_GARDENS)
                .map(crate::app::reading::vm::key)
            {
                this.open_reading(k, window, cx);
            }
        };
        match (scenario, n) {
            ("pf-reading", 0) => open_lin(self, window, cx),
            ("pf-reading", 1) => self.profile_for_context(window, cx),
            ("pf-blogroll", 0) => open_lin(self, window, cx),
            ("pf-blogroll", 1) => self.open_profile(LIN, window, cx),
            ("pf-blogroll", 2) => self.open_profile(TIDES, window, cx),
            ("pf-connections", 0) => open_lin(self, window, cx),
            ("pf-connections", 1) => self.open_profile(LIN, window, cx),
            ("pf-connections", 2) => self.set_profile_tab(Tab::Connections, cx),
            ("pf-posts", 0) => self.show_view(View::Reading, window, cx),
            ("pf-posts", 1) => self.open_profile(RUE, window, cx),
            ("pf-posts", 2) => self.set_profile_tab(Tab::Posts, cx),
            ("pf-feed", 0) => self.show_view(View::Reading, window, cx),
            ("pf-feed", 1) => self.open_profile(format!("{OMAR}feed.xml"), window, cx),
            ("pf-own", 1) => self.open_own_profile(window, cx),
            ("pf-ask", 1) => {
                self.ask_profile_url(window, cx);
                if let Some(i) = self.profiles.ask.clone() {
                    i.update(cx, |s, cx| {
                        s.set_value("https://tides.example.org/", window, cx)
                    });
                }
            }
            _ => {}
        }
        if n == 2 {
            crate::app::reading::demo::snapshot_later(window, cx);
        }
    }
}
