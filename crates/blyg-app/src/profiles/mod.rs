//! --- profiles --- (docs/SPEC.md § Profiles; mock: docs/prototype/profiles.html)
//!
//! A sheet that slides in from the right: who someone is (name, origin,
//! bio, links), and three lists: their **Blogroll**, their recent **Posts**
//! and their **Connections** (origins they quote, stub and fork). Follow =
//! the subscribe preview + subscribe in one step (private, client-local);
//! "Add to my blogroll" is a separate, deliberate step. Profile → profile
//! navigation keeps a stack with a back arrow.
//!
//! Privacy: a profile is fetched (by blyg-core, without the token) only when
//! it's opened here. Nothing in this module fetches in the background.
//!
//! A child module of `app` like `reading`, so it extends `MainView`; the
//! hooks in app.rs, studio/mod.rs, reading/list.rs, reading/mentions.rs,
//! keymap.rs and main.rs are marked `// --- profiles ---`.
//!
//! Hook for the reading body's web view (another branch): when its link
//! clicks come back to the app, a link to another blyg's origin (a quote's
//! provenance line) can call [`MainView::open_profile_from_link`] instead of
//! opening the browser; it returns false when the link isn't one.

mod demo;
mod view;
pub(crate) mod vm;

#[cfg(test)]
mod tests;

use std::collections::HashSet;

use blyg_core::Profile;
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::*;

use super::MainView;
use super::reading::View;
use vm::Tab;

gpui_kit::actions!(blygger, [ShowProfile, OpenProfile, MyProfile]);

/// One profile on the stack.
pub struct Page {
    /// What was opened (an origin, a permalink, a feed URL).
    pub url: String,
    /// The profile, once known (a cached copy shows while a fetch runs).
    pub profile: Option<Profile>,
    pub loading: bool,
    pub error: Option<String>,
    pub tab: Tab,
    /// The selected row of the current tab.
    pub sel: usize,
}

#[derive(Default)]
pub struct State {
    /// Open profiles, the last one on screen. Empty = the sheet is closed.
    pub stack: Vec<Page>,
    pub focus: Option<FocusHandle>,
    /// Bumped per open, for the slide-in animation.
    pub gen_: usize,
    /// ⇧⌘O "Open profile…": the URL input, and its error.
    pub ask: Option<Entity<InputState>>,
    pub ask_error: Option<String>,
    /// Follows / blogroll changes in flight (by URL or subscription id).
    pub busy: HashSet<String>,
    /// Where focus goes back to when the sheet closes.
    return_focus: Option<FocusHandle>,
}

impl State {
    pub fn is_open(&self) -> bool {
        !self.stack.is_empty()
    }

    /// Anything of ours drawn over the window (for suppressing native views).
    pub fn covers(&self) -> bool {
        self.is_open() || self.ask.is_some()
    }

    pub fn page(&self) -> Option<&Page> {
        self.stack.last()
    }

    fn page_mut(&mut self) -> Option<&mut Page> {
        self.stack.last_mut()
    }
}

// ================================================================ hooks

impl MainView {
    /// Hook: this module's actions on the root element.
    pub(super) fn profile_actions(
        &self,
        d: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        d.on_action(
            cx.listener(|this, _: &ShowProfile, window, cx| this.profile_for_context(window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &OpenProfile, window, cx| this.ask_profile_url(window, cx)),
        )
        .on_action(cx.listener(|this, _: &MyProfile, window, cx| this.open_own_profile(window, cx)))
    }

    /// Hook: the sheet, drawn over everything but toasts.
    pub(super) fn render_profile_overlay(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if let Some(ask) = self.profiles.ask.clone() {
            return Some(self.render_profile_ask(&ask, cx));
        }
        self.profiles
            .is_open()
            .then(|| self.render_profile_sheet(cx))
    }

    /// Hook: the sheet is open (native views must stay hidden under it).
    pub(super) fn profile_sheet_open(&self) -> bool {
        self.profiles.covers()
    }

    // ------------------------------------------------------------ opening

    /// ⌘I: the author of the reading item on screen; your own blyg
    /// anywhere else (your own post, the posts list). Again: close.
    pub(crate) fn profile_for_context(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.profiles.is_open() {
            self.close_profile(window, cx);
            return;
        }
        let reading_origin = match self.reading.view {
            View::Reading => self.reading.opened.as_ref().map(|o| o.item.origin.clone()),
            _ => None,
        };
        match reading_origin {
            Some(o) => self.open_profile(o, window, cx),
            None => self.open_own_profile(window, cx),
        }
    }

    /// Blyg › My Profile: what visitors see of your blyg.
    pub(crate) fn open_own_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.base_url.clone().or_else(|| self.backend.base_url()) {
            Some(own) => {
                self.profiles.stack.clear();
                self.open_profile(own, window, cx)
            }
            None => self.show_toast("Connect a blyg first", None, cx),
        }
    }

    /// Open the profile at `url` on top of whatever is open (profile →
    /// profile navigation), showing the cached copy at once and fetching
    /// (only if stale) in the background.
    pub(crate) fn open_profile(
        &mut self,
        url: impl Into<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let url = url.into();
        if self.profiles.page().is_some_and(|p| p.url == url) {
            return;
        }
        if !self.profiles.is_open() {
            self.profiles.return_focus = window.focused(cx);
            self.profiles.gen_ += 1;
        }
        self.profiles.ask = None;
        let focus = self
            .profiles
            .focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        let cached = self.backend.cached_profile(&url);
        self.profiles.stack.push(Page {
            url: url.clone(),
            profile: cached,
            loading: true,
            error: None,
            tab: Tab::Blogroll,
            sel: 0,
        });
        self.fix_tab();
        window.focus(&focus, cx);
        self.fetch_profile(url, false, cx);
        cx.notify();
    }

    fn fetch_profile(&mut self, url: String, refresh: bool, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        let u = url.clone();
        let task = cx.background_spawn(async move { backend.profile(&u, refresh) });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |v, cx| {
                // Only the page it was for, if it's still open.
                let Some(page) = v.profiles.stack.iter_mut().rev().find(|p| p.url == url) else {
                    return;
                };
                page.loading = false;
                match r {
                    Ok(p) => {
                        page.profile = Some(p);
                        page.error = None;
                    }
                    Err(e) => page.error = Some(e.to_string()),
                }
                v.fix_tab();
                cx.notify();
            });
        })
        .detach();
    }

    /// A feed has only Posts: move the tab onto one it has.
    fn fix_tab(&mut self) {
        if let Some(page) = self.profiles.page_mut()
            && let Some(p) = &page.profile
        {
            let tabs = Tab::all_for(p);
            if !tabs.contains(&page.tab) {
                page.tab = tabs[0];
                page.sel = 0;
            }
        }
    }

    pub(crate) fn refresh_profile(&mut self, cx: &mut Context<Self>) {
        if let Some(page) = self.profiles.page_mut() {
            page.loading = true;
            let url = page.url.clone();
            self.fetch_profile(url, true, cx);
            cx.notify();
        }
    }

    /// The back arrow: the previous profile.
    pub(crate) fn profile_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.profiles.stack.len() > 1 {
            self.profiles.stack.pop();
            cx.notify();
        } else {
            self.close_profile(window, cx);
        }
    }

    pub(crate) fn close_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.profiles.stack.clear();
        self.profiles.ask = None;
        self.profiles.ask_error = None;
        match self.profiles.return_focus.take() {
            Some(f) => window.focus(&f, cx),
            None if self.reading.view != View::Posts => window.focus(&self.reading.focus, cx),
            None => self.focus_after_sheet(window, cx),
        }
        cx.notify();
    }

    /// The hook for a link clicked inside rendered post HTML (see the module
    /// docs): opens the profile when `url` is another blyg's origin we know
    /// of (a subscription or a post we hold); false = open it as a link.
    #[allow(dead_code)] // wired up by the reading body's web view
    pub(crate) fn open_profile_from_link(
        &mut self,
        url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        use blyg_core::profile::{normalize_origin, same_origin};
        let Some(o) = normalize_origin(url) else {
            return false;
        };
        // Only a link to the origin itself (a provenance line's "ada.example.net"),
        // not to one of its posts, and only an origin we already know of.
        let known = self.reading.subs.iter().any(|s| same_origin(&s.origin, &o))
            || self.reading.rows.iter().any(|r| same_origin(&r.origin, &o));
        let take = known && same_origin(url, &o);
        if take {
            self.open_profile(o, window, cx);
        }
        take
    }

    // ------------------------------------------------------------ ⇧⌘O

    pub(crate) fn ask_profile_url(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("A blyg, a post, or a feed: https://…")
        });
        let sub = cx.subscribe_in(&input, window, |this, input, ev, window, cx| match ev {
            InputEvent::PressEnter { .. } => {
                let url = input.read(cx).value().trim().to_string();
                match blyg_core::profile::clean_url(&url) {
                    Some(u) => {
                        this.profiles.stack.clear();
                        this.open_profile(u, window, cx)
                    }
                    None => {
                        this.profiles.ask_error = Some("That isn't a web address.".into());
                        cx.notify();
                    }
                }
            }
            InputEvent::Change => {
                this.profiles.ask_error = None;
                cx.notify();
            }
            _ => {}
        });
        self._subs.push(sub);
        if !self.profiles.is_open() {
            self.profiles.return_focus = window.focused(cx);
        }
        self.profiles.stack.clear();
        input.update(cx, |s, cx| s.focus(window, cx));
        self.profiles.ask = Some(input);
        self.profiles.ask_error = None;
        cx.notify();
    }

    // ------------------------------------------------------------ actions

    /// Follow: the subscribe preview, then subscribe, in one step. Following
    /// is private; unfollowing lives in Subscriptions.
    pub(crate) fn follow_url(&mut self, url: String, cx: &mut Context<Self>) {
        let subs = self.backend.subscriptions();
        if let Some(s) = vm::followed(&url, &subs) {
            self.show_toast(
                format!("Already following {}", s.title),
                Some("Unfollow in Subscriptions (⇧⌘S)".into()),
                cx,
            );
            return;
        }
        if !self.profiles.busy.insert(url.clone()) {
            return;
        }
        cx.notify();
        let backend = self.backend.clone();
        let u = url.clone();
        let task = cx.background_spawn(async move {
            let preview = backend.preview_subscription(&u)?;
            let title = (!preview.title.trim().is_empty()).then_some(preview.title.clone());
            backend.subscribe(&u, title.as_deref())
        });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |v, cx| {
                v.profiles.busy.remove(&url);
                v.reading.subs = v.backend.subscriptions();
                match r {
                    Ok(s) => v.show_toast(
                        format!("Following {}", s.title),
                        Some("Private to you · unfollow in Subscriptions".into()),
                        cx,
                    ),
                    Err(e) => v.show_toast(
                        format!("Couldn't follow {}: {e}", vm::short(&url)),
                        None,
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// "Add to my blogroll": follow first if needed, then set the flag.
    pub(crate) fn add_to_blogroll(&mut self, url: String, cx: &mut Context<Self>) {
        if !self.profiles.busy.insert(format!("roll:{url}")) {
            return;
        }
        cx.notify();
        let backend = self.backend.clone();
        let u = url.clone();
        let task = cx.background_spawn(async move {
            let subs = backend.subscriptions();
            let sub = match vm::followed(&u, &subs) {
                Some(s) => s.clone(),
                None => {
                    let preview = backend.preview_subscription(&u)?;
                    let title = (!preview.title.trim().is_empty()).then_some(preview.title.clone());
                    backend.subscribe(&u, title.as_deref())?
                }
            };
            backend.set_subscription(&sub.id, Some(true), None)?;
            Ok::<_, blyg_core::CoreError>(sub)
        });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |v, cx| {
                v.profiles.busy.remove(&format!("roll:{url}"));
                v.reading.subs = v.backend.subscriptions();
                match r {
                    Ok(s) => v.show_toast(
                        format!("{} is in your blogroll", s.title),
                        Some("Your blogroll is public (blogroll.opml)".into()),
                        cx,
                    ),
                    Err(e) => v.show_toast(format!("Couldn't add it: {e}"), None, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Your own profile's blogroll toggles.
    pub(crate) fn toggle_own_blogroll(&mut self, sub_id: String, on: bool, cx: &mut Context<Self>) {
        if !self.profiles.busy.insert(sub_id.clone()) {
            return;
        }
        let backend = self.backend.clone();
        let id = sub_id.clone();
        let task =
            cx.background_spawn(async move { backend.set_subscription(&id, Some(on), None) });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |v, cx| {
                v.profiles.busy.remove(&sub_id);
                v.reading.subs = v.backend.subscriptions();
                if let Err(e) = r {
                    v.show_toast(format!("Couldn't change your blogroll: {e}"), None, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn profile_button(&mut self, b: vm::Button, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.profiles.page().and_then(|p| p.profile.clone()) else {
            return;
        };
        let target = p
            .feed_url
            .clone()
            .filter(|_| p.kind == blyg_core::ProfileKind::Feed);
        let follow = target.unwrap_or_else(|| p.origin.clone());
        match b {
            vm::Button::Follow | vm::Button::Following => self.follow_url(follow, cx),
            vm::Button::OpenSite => cx.open_url(&p.origin),
            vm::Button::AddToBlogroll => self.add_to_blogroll(follow, cx),
            vm::Button::InBlogroll => self.show_toast(
                "Already in your blogroll",
                Some("Change it in Subscriptions (⇧⌘S)".into()),
                cx,
            ),
            vm::Button::EditSite => {
                self.close_profile(window, cx);
                self.open_site_settings(window, cx);
            }
            vm::Button::Refresh => self.refresh_profile(cx),
        }
    }

    // ------------------------------------------------------------ keys

    fn current_rows(&self) -> Vec<vm::Row> {
        let Some(page) = self.profiles.page() else {
            return vec![];
        };
        let Some(p) = &page.profile else {
            return vec![];
        };
        vm::rows(p, page.tab, &self.reading.subs, self.now)
    }

    pub(crate) fn set_profile_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if let Some(page) = self.profiles.page_mut() {
            page.tab = tab;
            page.sel = 0;
        }
        cx.notify();
    }

    /// ⏎ on a row: its profile, or the post on the web.
    pub(crate) fn activate_profile_row(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(r) = self.current_rows().into_iter().nth(ix) else {
            return;
        };
        if let Some(u) = r.profile_url {
            self.open_profile(u, window, cx);
        } else if let Some(u) = r.open_url {
            cx.open_url(&u);
        }
    }

    fn profile_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        if k.modifiers.platform || k.modifiers.control || k.modifiers.alt {
            return;
        }
        let n = self.current_rows().len();
        let handled = match k.key.as_str() {
            "escape" => {
                self.close_profile(window, cx);
                true
            }
            "down" | "j" => {
                if let Some(p) = self.profiles.page_mut()
                    && n > 0
                {
                    p.sel = (p.sel + 1).min(n - 1);
                }
                cx.notify();
                true
            }
            "up" | "k" => {
                if let Some(p) = self.profiles.page_mut() {
                    p.sel = p.sel.saturating_sub(1);
                }
                cx.notify();
                true
            }
            "enter" => {
                let ix = self.profiles.page().map(|p| p.sel).unwrap_or(0);
                self.activate_profile_row(ix, window, cx);
                true
            }
            "f" => {
                let ix = self.profiles.page().map(|p| p.sel).unwrap_or(0);
                if let Some(u) = self
                    .current_rows()
                    .into_iter()
                    .nth(ix)
                    .and_then(|r| r.follow_url)
                {
                    self.follow_url(u, cx);
                }
                true
            }
            "left" | "backspace" => {
                if self.profiles.stack.len() > 1 {
                    self.profile_back(window, cx);
                }
                true
            }
            "tab" | "right" => {
                if let Some(page) = self.profiles.page()
                    && let Some(p) = &page.profile
                {
                    let tabs = Tab::all_for(p);
                    let i = tabs.iter().position(|t| *t == page.tab).unwrap_or(0);
                    let next = tabs[(i + 1) % tabs.len()];
                    self.set_profile_tab(next, cx);
                }
                true
            }
            _ => false,
        };
        if handled {
            cx.stop_propagation();
        }
    }
}
