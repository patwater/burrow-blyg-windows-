//! What Burrow tells you about your blyg's server.
//!
//! The "your blyg's server needs updating" notice: Burrow needs
//! blygger-studio 0.9 or later, and an older server gets nothing pushed to it
//! (see `CoreError::ServerOutdated`). A toast when sync first notices, then a
//! status-bar notice for as long as it stays true.
//!
//! The "this blyg runs stock blygger-studio" sheet: once per local database,
//! when a reading pull first finds a server without the extensions in
//! docs/SERVER.md. It says what works and what's limited, and why.
//!
//! A child module of `app` (like the update notice) so it can use
//! `MainView`'s toast and sheet.

use gpui_kit::*;

use super::MainView;

/// Where the notice's link goes: the server requirements, which link on to
/// upstream's upgrade guide.
pub const SERVER_HELP_URL: &str = "https://aneeshsathe.github.io/blygger-desktop/server.html";

/// The toast, once per session.
pub const OUTDATED_TOAST: &str = "Your blyg's server needs updating";
pub const OUTDATED_TOAST_SUB: &str =
    "Burrow needs blygger-studio 0.9 or later. Nothing syncs until the server is updated.";

/// The status bar's text while the server is outdated.
pub const OUTDATED_NOTICE: &str = "Server needs updating · nothing syncs";

/// The stock-server sheet.
pub const LIMITS_TITLE: &str = "Your blyg runs stock blygger-studio";
pub const LIMITS_INTRO: &str = "Writing, publishing, versions, subscriptions and your reading \
     list all work. A few things need a newer studio, or extensions this server doesn't have:";
/// What's limited, and how Burrow copes. Also in the About window.
pub const LIMITS: [&str; 4] = [
    "Read state stays on this Mac. Posts you read here still show as unread on your other Macs.",
    "Before studio 0.28: AI disclosure for text generated in Burrow. Before you publish such \
     text, Burrow says it will go out without the disclosure, and lets you cancel.",
    "Before studio 0.26: an image you paste into a draft and then delete stays on the server.",
    "Before studio 0.18: who a post replies to, or forks, appears once Burrow has fetched it \
     from the author's blyg.",
];
pub const LIMITS_OUTRO: &str =
    "Generation on your blyg's own server, if you use it, records its disclosure itself.";

impl MainView {
    /// A reading pull just found a stock server (first time for this
    /// database): explain, unless another sheet is up; then a toast points
    /// at the About window, which lists the same.
    pub(super) fn server_limited(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sheet.is_some() {
            self.show_toast(
                LIMITS_TITLE,
                Some("A few features are limited: see Burrow › About".into()),
                cx,
            );
            return;
        }
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.sheet_gen += 1;
        self.sheet = Some(super::Sheet::ServerLimits { focus });
        cx.notify();
    }

    /// Sync just found an outdated server.
    pub(super) fn server_outdated(&mut self, cx: &mut Context<Self>) {
        self.show_toast(OUTDATED_TOAST, Some(OUTDATED_TOAST_SUB.into()), cx);
    }

    /// "Server needs updating · nothing syncs · How to update", while true.
    pub(super) fn render_server_notice(&self) -> Option<AnyElement> {
        if !self.backend.server_outdated() {
            return None;
        }
        // --- themes --- on the status bar's own ground, warning-coloured.
        let warn = self.palette.on_status_text(self.palette.warn);
        let link = self.palette.on_status_text(self.palette.accent);
        Some(
            div()
                .id("server-notice")
                .flex()
                .items_center()
                .gap(px(6.))
                .min_w_0()
                .overflow_hidden()
                .child(div().truncate().text_color(warn).child(OUTDATED_NOTICE))
                .child("·")
                .child(
                    div()
                        .id("server-notice-link")
                        .cursor_pointer()
                        .text_color(link)
                        .hover(|s| s.underline())
                        .child("How to update")
                        .on_click(|_, _, cx| cx.open_url(SERVER_HELP_URL)),
                )
                .into_any_element(),
        )
    }
}
