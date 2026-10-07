//! --- reader folders --- The Reader's sources pane (NetNewsWire-style):
//! smart feeds (All unread, Today, Thumbed, All), then the user's folders
//! with their subscriptions, then the subscriptions in no folder. Picking a
//! source filters the post list beside it; the post itself is on the right.
//!
//! Folders are local only (`Backend::folders` and friends, the store's
//! `folders` / `folder_members` tables): they never reach the blyg, and they
//! aren't the server's hoppers. A subscription is filed by dragging it onto
//! a folder, from its context menu ("Move to folder ›"), or on the
//! Subscriptions screen.
//!
//! Keys (Reader mode, `reader_key`): ←/→ move between the panes, ↑/↓ within
//! one, j/k go to the next/previous post from anywhere, Space pages through
//! the post and then opens the next unread one, ⏎ goes right, esc goes back
//! to the list. [ / ] step through the versions (←/→ did that before the
//! panes). ⌥⌘S shows or hides the pane; its right edge drags to resize.

use blyg_core::Backend;
use gpui_kit::base::input::{Escape, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::sources_vm::{self, Entry, Pane, Smart, Source};
use super::stream_vm::ReadMode;
use super::{RSheet, View};
use crate::app::MainView;
use crate::theme::Rule; // --- themes --- dividers

gpui_kit::actions!(blygger, [ToggleSources, NewFolder, CheckFeeds]);

pub const SOURCES_W: f32 = 210.;
const MIN_W: f32 = 150.;
const MAX_W: f32 = 380.;

// Small line icons (drawn for this pane, 24×24, stroked in currentColor).
const ICON_UNREAD: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9"><circle cx="12" cy="12" r="8"/><circle cx="12" cy="12" r="3" fill="currentColor"/></svg>"#;
const ICON_TODAY: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"><circle cx="12" cy="12" r="4"/><path d="M12 2.5v2M12 19.5v2M2.5 12h2M19.5 12h2M5.3 5.3l1.4 1.4M17.3 17.3l1.4 1.4M5.3 18.7l1.4-1.4M17.3 6.7l1.4-1.4"/></svg>"#;
const ICON_THUMB: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linejoin="round"><path d="M7 10.5V20H4.5v-9.5z"/><path d="M7 10.5l3.6-6.5c1.4 0 2.4 1 2.2 2.5L12.3 10H18a2 2 0 0 1 2 2.3l-1.1 5.9A2.2 2.2 0 0 1 16.7 20H7"/></svg>"#;
const ICON_ALL: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"><path d="M8.5 6.5h11M8.5 12h11M8.5 17.5h11"/><circle cx="4.5" cy="6.5" r=".8" fill="currentColor"/><circle cx="4.5" cy="12" r=".8" fill="currentColor"/><circle cx="4.5" cy="17.5" r=".8" fill="currentColor"/></svg>"#;
const ICON_FOLDER: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"><path d="M3 6.5A1.5 1.5 0 0 1 4.5 5H9l2 2.2h8.5A1.5 1.5 0 0 1 21 8.7v8.8a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 17.5z"/></svg>"#;

fn smart_icon(s: Smart) -> &'static [u8] {
    match s {
        Smart::Unread => ICON_UNREAD,
        Smart::Today => ICON_TODAY,
        Smart::Thumbed => ICON_THUMB,
        Smart::All => ICON_ALL,
    }
}

// ---------------------------------------------------------------- state

/// What a context menu is about.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuTarget {
    /// The pane's background or a heading.
    Pane,
    Folder(String),
    Sub(String),
    /// --- read/unread --- a post in the list (and the picks, when it's
    /// among them).
    Posts(super::vm::Key),
}

/// A context menu, at a window position.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceMenu {
    pub target: MenuTarget,
    pub at: Point<Pixels>,
    /// The "Move to folder ›" list is showing.
    pub folders: bool,
}

/// What a menu item does (tests run these directly).
#[derive(Debug, Clone, PartialEq)]
pub enum MenuCmd {
    NewFolder,
    Rename(String),
    Delete(String),
    MoveUp(String),
    MoveDown(String),
    /// Show the folder list of a subscription's menu.
    MoveToFolder,
    /// File a subscription (`None` = no folder).
    File(String, Option<String>),
    /// A new folder, then file the subscription in it.
    NewFolderFor(String),
    Profile(String),
    // --- subscription names & checks --- (studio 0.30)
    /// Rename… a subscription (a name of your own).
    RenameSub(String),
    /// "Use the blyg's own name": the name follows the source again.
    FollowSource(String),
    /// Check one subscription now.
    CheckNow(String),
    /// Check all feeds now (⇧⌘R).
    CheckAll,
    // --- read/unread ---
    /// Mark these posts read (`true`) or unread.
    Mark(Vec<super::vm::Key>, bool),
}

/// A subscription being dragged onto a folder.
#[derive(Debug, Clone)]
pub struct DraggedSub {
    pub id: String,
    pub name: String,
}

/// A folder being dragged to a new place in the order.
#[derive(Debug, Clone)]
pub struct DraggedFolder {
    pub id: String,
    pub name: String,
}

/// The sources pane's right edge, being dragged.
#[derive(Debug, Clone)]
struct DraggedEdge;

/// What follows the pointer while dragging a subscription or a folder.
struct DragChip(String);

impl Render for DragChip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.))
            .py(px(3.))
            .rounded(px(6.))
            .bg(gpui_kit::black().opacity(0.78))
            .text_color(gpui_kit::white())
            .font_family("Inter")
            .text_size(px(11.5))
            .child(self.0.clone())
    }
}

impl Render for DraggedEdge {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// Whether the pane was left hidden (`state.json`).
pub fn load_open(data_dir: Option<&std::path::Path>) -> bool {
    !data_dir
        .map(blyg_core::state::AppState::load)
        .is_some_and(|s| s.reader_sources_hidden)
}

pub fn load_width(data_dir: Option<&std::path::Path>) -> f32 {
    data_dir
        .map(blyg_core::state::AppState::load)
        .and_then(|s| s.reader_sources_width)
        .map(|w| (w as f32).clamp(MIN_W, MAX_W))
        .unwrap_or(SOURCES_W)
}

/// (origin, avatar URL) for every cached profile that has an avatar.
pub fn avatars(backend: &dyn Backend) -> Vec<(String, String)> {
    backend
        .cached_profiles()
        .into_iter()
        .filter_map(|p| p.avatar.map(|a| (p.origin, a)))
        .collect()
}

impl MainView {
    fn sources_data_dir(&self, cx: &App) -> Option<std::path::PathBuf> {
        cx.try_global::<crate::connection::Connection>()
            .map(|c| c.data_dir.clone())
    }

    pub(super) fn sources_actions(
        &self,
        d: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        d.on_action(
            cx.listener(|this, _: &ToggleSources, window, cx| this.toggle_sources(window, cx)),
        )
        .on_action(cx.listener(|this, _: &NewFolder, window, cx| {
            this.open_folder_sheet(None, None, window, cx)
        }))
        .on_action(
            cx.listener(|this, _: &CheckFeeds, window, cx| this.check_feeds(None, window, cx)),
        )
    }

    /// ⌥⌘S: show or hide the sources pane (and go to the Reader).
    pub(crate) fn toggle_sources(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let on_reader = self.reading.view == View::Reading && self.reading.mode == ReadMode::Reader;
        if !on_reader {
            self.reading.sources_open = true;
            self.set_read_mode(ReadMode::Reader, window, cx);
        } else {
            self.reading.sources_open = !self.reading.sources_open;
            if !self.reading.sources_open && self.reading.pane == Pane::Sources {
                self.reading.pane = Pane::List;
            }
        }
        let hidden = !self.reading.sources_open;
        if let Some(d) = self.sources_data_dir(cx) {
            let _ = blyg_core::state::AppState::update(&d, |s| s.reader_sources_hidden = hidden);
        }
        cx.notify();
    }

    // ------------------------------------------------------------ sources

    /// Everything the pane lists now.
    pub(crate) fn source_entries(&self) -> Vec<Entry> {
        sources_vm::entries(
            &self.reading.folders,
            &self.reading.filed,
            &self.reading.subs,
            &self.reading.rows,
            &self.reading.collapsed,
            self.now,
        )
    }

    /// Show `source`'s posts in the list. The open post stays when it's
    /// among them; otherwise nothing is open (nothing is marked read just by
    /// picking a source).
    pub(crate) fn select_source(&mut self, source: Source, cx: &mut Context<Self>) {
        if self.reading.source != source {
            self.reading.source = source;
            self.reading.sticky.clear();
            self.reading.refilter();
            let keep = self
                .reading
                .sel
                .as_ref()
                .and_then(|k| self.reading.shown_pos(k));
            match keep {
                Some(ix) => self
                    .reading
                    .list_scroll
                    .scroll_to_item(ix, ScrollStrategy::Nearest),
                None => {
                    self.reading.sel = None;
                    self.reading.opened = None;
                    self.reading.sticky.clear();
                    self.reading
                        .list_scroll
                        .scroll_to_item(0, ScrollStrategy::Top);
                }
            }
        }
        cx.notify();
    }

    /// Re-read folders after a change and refilter the list.
    fn reload_folders(&mut self, cx: &mut Context<Self>) {
        self.reading.folders = self.backend.folders();
        self.reading.filed = self.backend.subscription_folders();
        if let Source::Folder(id) = &self.reading.source
            && !self.reading.folders.iter().any(|f| &f.id == id)
        {
            self.reading.source = Source::default();
        }
        self.reading.refilter();
        cx.notify();
    }

    /// File a subscription in a folder (`None`: no folder).
    pub(crate) fn file_subscription(
        &mut self,
        sub_id: &str,
        folder: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        if self.reading.filed.get(sub_id).map(String::as_str) == folder {
            return;
        }
        match self.backend.set_subscription_folder(sub_id, folder) {
            Ok(()) => {
                let name = self
                    .reading
                    .subs
                    .iter()
                    .find(|s| s.id == sub_id)
                    .map(sources_vm::sub_name)
                    .unwrap_or_default();
                let msg = match folder.and_then(|f| self.reading.folders.iter().find(|x| x.id == f))
                {
                    Some(f) => format!("Moved {name} to {}", f.name),
                    None => format!("{name} is in no folder now"),
                };
                self.reload_folders(cx);
                self.show_toast(msg, None, cx);
            }
            Err(e) => self.show_toast(format!("Couldn't move it: {e}"), None, cx),
        }
    }

    /// Move a folder to `index` (drag and drop, Move Up / Move Down).
    pub(crate) fn reorder_folder(&mut self, id: &str, index: usize, cx: &mut Context<Self>) {
        match self.backend.move_folder(id, index) {
            Ok(()) => self.reload_folders(cx),
            Err(e) => self.show_toast(format!("Couldn't move the folder: {e}"), None, cx),
        }
    }

    pub(crate) fn delete_folder(&mut self, id: &str, cx: &mut Context<Self>) {
        let name = self
            .reading
            .folders
            .iter()
            .find(|f| f.id == id)
            .map(|f| f.name.clone())
            .unwrap_or_default();
        match self.backend.delete_folder(id) {
            Ok(()) => {
                self.reading.collapsed.remove(id);
                self.reload_folders(cx);
                self.show_toast(
                    format!("Deleted the folder “{name}”"),
                    Some("Its subscriptions are in no folder now".into()),
                    cx,
                );
            }
            Err(e) => self.show_toast(format!("Couldn't delete the folder: {e}"), None, cx),
        }
    }

    /// Run a context-menu command (the menu closes, except for the
    /// "Move to folder ›" step).
    pub(crate) fn run_menu_cmd(
        &mut self,
        cmd: MenuCmd,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if cmd == MenuCmd::MoveToFolder {
            if let Some(m) = self.reading.src_menu.as_mut() {
                m.folders = true;
            }
            cx.notify();
            return;
        }
        self.reading.src_menu = None;
        let up = matches!(cmd, MenuCmd::MoveUp(_));
        match cmd {
            MenuCmd::NewFolder => self.open_folder_sheet(None, None, window, cx),
            MenuCmd::Rename(id) => self.open_folder_sheet(Some(id), None, window, cx),
            MenuCmd::Delete(id) => self.delete_folder(&id, cx),
            MenuCmd::MoveUp(id) | MenuCmd::MoveDown(id) => {
                if let Some(i) = self.reading.folders.iter().position(|f| f.id == id) {
                    let to = if up { i.saturating_sub(1) } else { i + 1 };
                    self.reorder_folder(&id, to, cx);
                }
            }
            MenuCmd::File(sub, folder) => self.file_subscription(&sub, folder.as_deref(), cx),
            MenuCmd::NewFolderFor(sub) => self.open_folder_sheet(None, Some(sub), window, cx),
            MenuCmd::Profile(origin) => self.open_profile(origin, window, cx),
            MenuCmd::MoveToFolder => {}
            MenuCmd::RenameSub(id) => self.open_sub_name_sheet(id, window, cx),
            MenuCmd::FollowSource(id) => self.follow_source_name(id, window, cx),
            MenuCmd::CheckNow(id) => self.check_feeds(Some(id), window, cx),
            MenuCmd::CheckAll => self.check_feeds(None, window, cx),
            MenuCmd::Mark(keys, read) => self.mark_posts(keys, read, cx),
        }
        cx.notify();
    }

    /// The items of the open menu: (label, command, checked, enabled).
    pub(crate) fn menu_items(&self) -> Vec<(String, Option<MenuCmd>, bool)> {
        let Some(m) = self.reading.src_menu.as_ref() else {
            return vec![];
        };
        let sep = || ("".to_string(), None, false);
        match &m.target {
            MenuTarget::Pane => vec![
                ("New Folder…".into(), Some(MenuCmd::NewFolder), false),
                sep(),
                ("Check All Feeds Now".into(), Some(MenuCmd::CheckAll), false),
            ],
            MenuTarget::Posts(key) => self
                .post_menu_items(key)
                .into_iter()
                .map(|(label, cmd)| (label, Some(cmd), false))
                .collect(),
            MenuTarget::Folder(id) => {
                let i = self.reading.folders.iter().position(|f| &f.id == id);
                let last = self.reading.folders.len().saturating_sub(1);
                let mut v = vec![("Rename…".into(), Some(MenuCmd::Rename(id.clone())), false)];
                if i.is_some_and(|i| i > 0) {
                    v.push(("Move Up".into(), Some(MenuCmd::MoveUp(id.clone())), false));
                }
                if i.is_some_and(|i| i < last) {
                    v.push((
                        "Move Down".into(),
                        Some(MenuCmd::MoveDown(id.clone())),
                        false,
                    ));
                }
                v.push((
                    "Delete Folder".into(),
                    Some(MenuCmd::Delete(id.clone())),
                    false,
                ));
                v.push(sep());
                v.push(("New Folder…".into(), Some(MenuCmd::NewFolder), false));
                v
            }
            MenuTarget::Sub(sub) if m.folders => {
                let cur = self.reading.filed.get(sub);
                let mut v: Vec<_> = self
                    .reading
                    .folders
                    .iter()
                    .map(|f| {
                        (
                            f.name.clone(),
                            Some(MenuCmd::File(sub.clone(), Some(f.id.clone()))),
                            cur == Some(&f.id),
                        )
                    })
                    .collect();
                v.push((
                    "No folder".into(),
                    Some(MenuCmd::File(sub.clone(), None)),
                    cur.is_none_or(|c| !self.reading.folders.iter().any(|f| &f.id == c)),
                ));
                v.push(sep());
                v.push((
                    "New Folder…".into(),
                    Some(MenuCmd::NewFolderFor(sub.clone())),
                    false,
                ));
                v
            }
            MenuTarget::Sub(sub) => {
                let held = self.reading.subs.iter().find(|s| &s.id == sub);
                let origin = held.map(|s| s.origin.clone());
                let mut v = vec![(
                    "Move to folder ›".into(),
                    Some(MenuCmd::MoveToFolder),
                    false,
                )];
                if let Some(o) = origin {
                    v.push(("Profile".into(), Some(MenuCmd::Profile(o)), false));
                }
                // --- subscription names & checks --- (studio 0.30)
                v.push(sep());
                v.push((
                    "Rename…".into(),
                    Some(MenuCmd::RenameSub(sub.clone())),
                    false,
                ));
                if held.is_some_and(|s| !s.title_follows_source) {
                    v.push((
                        own_name_label(held.map(|s| s.kind)).into(),
                        Some(MenuCmd::FollowSource(sub.clone())),
                        false,
                    ));
                }
                v.push((
                    "Check Now".into(),
                    Some(MenuCmd::CheckNow(sub.clone())),
                    false,
                ));
                v.push(sep());
                v.push(("New Folder…".into(), Some(MenuCmd::NewFolder), false));
                v
            }
        }
    }

    pub(crate) fn open_source_menu(
        &mut self,
        target: MenuTarget,
        at: Point<Pixels>,
        folders: bool,
        cx: &mut Context<Self>,
    ) {
        self.reading.src_menu = Some(SourceMenu {
            target,
            at,
            folders,
        });
        cx.notify();
    }

    // ------------------------------------------------------------ names & checks

    /// Check feeds now: one subscription (`Check Now`: a blyg's index is
    /// reconciled at once, a feed is polled with the rest), or all of them
    /// (⇧⌘R, `Check All Feeds Now`: the blyg polls every subscription in
    /// the background). A pull follows, and another a few seconds later for
    /// what the background poll brings.
    pub(crate) fn check_feeds(
        &mut self,
        sub: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let backend = self.backend.clone();
        let one = sub.clone();
        let name = sub
            .as_ref()
            .and_then(|id| self.reading.subs.iter().find(|s| &s.id == id))
            .map(sources_vm::sub_name);
        self.show_toast(
            match &name {
                Some(n) => format!("Checking {n}…"),
                None => "Checking all feeds…".into(),
            },
            None,
            cx,
        );
        let task = cx.background_spawn(async move {
            let r = match &one {
                Some(id) => backend.check_subscription(id).map(|_| None),
                None => backend.poll_subscriptions().map(Some),
            };
            if r.is_ok() {
                let _ = backend.sync_now();
            }
            r
        });
        cx.spawn_in(window, async move |this, cx| {
            let r = task.await;
            let again = r.is_ok();
            let _ = this.update_in(cx, |v, _, cx| match r {
                Ok(Some(n)) => v.show_toast(
                    format!("Checking {n} feed{}", if n == 1 { "" } else { "s" }),
                    Some("New posts arrive as the blyg finds them".into()),
                    cx,
                ),
                Ok(None) => v.show_toast(
                    format!("Checked {}", name.as_deref().unwrap_or("it")),
                    None,
                    cx,
                ),
                Err(e) => v.show_toast(format!("Couldn't check the feeds: {e}"), None, cx),
            });
            if again {
                let backend = this.update(cx, |v, _| v.backend.clone()).ok();
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(5))
                    .await;
                if let Some(b) = backend {
                    let _ = cx.background_spawn(async move { b.sync_now() }).await;
                }
            }
        })
        .detach();
    }

    /// "Use the blyg's own name": the name follows the source again
    /// (`title: null`), and a check brings it now rather than at the next
    /// poll.
    pub(crate) fn follow_source_name(
        &mut self,
        sub: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let backend = self.backend.clone();
        let id = sub.clone();
        let task = cx.background_spawn(async move {
            backend.rename_subscription(&id, None)?;
            let _ = backend.poll_subscriptions();
            let _ = backend.sync_now();
            Ok::<(), blyg_core::CoreError>(())
        });
        cx.spawn_in(window, async move |this, cx| {
            let r = task.await;
            let _ = this.update_in(cx, |v, _, cx| {
                v.reading.subs = v.backend.subscriptions();
                match r {
                    Ok(()) => v.show_toast(
                        "It takes the blyg's own name again",
                        Some("Renamed there, it's renamed here".into()),
                        cx,
                    ),
                    Err(e) => v.show_toast(format!("Couldn't change the name: {e}"), None, cx),
                }
            });
        })
        .detach();
    }

    /// Rename… a subscription: a sheet like the folders' (⏎ renames).
    pub(crate) fn open_sub_name_sheet(
        &mut self,
        sub: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self
            .reading
            .subs
            .iter()
            .find(|s| s.id == sub)
            .map(|s| s.title.clone())
            .unwrap_or_default();
        let input = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("A name of your own");
            s.set_value(current, window, cx);
            s
        });
        let ev = cx.subscribe_in(&input, window, |this, _, ev, window, cx| match ev {
            InputEvent::PressEnter { .. } => this.commit_sub_name_sheet(window, cx),
            InputEvent::Change => {
                if let Some(RSheet::SubName { error, .. }) = this.reading.sheet.as_mut()
                    && error.take().is_some()
                {
                    cx.notify();
                }
            }
            _ => {}
        });
        self._subs.push(ev);
        input.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        self.open_reading_sheet(
            RSheet::SubName {
                input,
                sub,
                error: None,
                busy: false,
            },
            cx,
        );
    }

    /// ⏎ in the rename sheet.
    pub(crate) fn commit_sub_name_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(RSheet::SubName {
            input, sub, busy, ..
        }) = self.reading.sheet.as_mut()
        else {
            return;
        };
        if *busy {
            return;
        }
        let name = input.read(cx).value().trim().to_string();
        if name.is_empty() {
            if let Some(RSheet::SubName { error, .. }) = self.reading.sheet.as_mut() {
                *error = Some("A name can't be empty".into());
            }
            return cx.notify();
        }
        *busy = true;
        let (backend, id) = (self.backend.clone(), sub.clone());
        let task =
            cx.background_spawn(async move { backend.rename_subscription(&id, Some(&name)) });
        cx.spawn_in(window, async move |this, cx| {
            let r = task.await;
            let _ = this.update_in(cx, |v, window, cx| match r {
                Ok(()) => {
                    v.close_reading_sheet(window, cx);
                    v.reading.subs = v.backend.subscriptions();
                    v.reading.refilter();
                    v.show_toast(
                        "Renamed",
                        Some("Your name stays when the blyg renames itself".into()),
                        cx,
                    );
                }
                Err(e) => {
                    if let Some(RSheet::SubName { error, busy, .. }) = v.reading.sheet.as_mut() {
                        *busy = false;
                        *error = Some(match e {
                            blyg_core::CoreError::Rejected { message, .. } => message,
                            e => e.to_string(),
                        });
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn render_sub_name_sheet(
        &self,
        sheet: &RSheet,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette.on_page();
        let RSheet::SubName {
            input,
            sub,
            error,
            busy,
        } = sheet
        else {
            return div().into_any_element();
        };
        let follows = self
            .reading
            .subs
            .iter()
            .find(|s| &s.id == sub)
            .is_some_and(|s| s.title_follows_source);
        let id = sub.clone();
        div()
            .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                cx.stop_propagation();
                this.close_reading_sheet(window, cx);
            }))
            .child(self.sheet_heading("Rename Subscription"))
            .child(div().mb(px(8.)).text_color(p.muted).child(if follows {
                "It goes by the blyg's own name now. A name of your own stays when the blyg renames itself."
            } else {
                "It goes by a name of your own."
            }))
            .child(self.input_box(
                gpui_kit::base::input::Input::new(input).into_any_element(),
                error.is_some(),
            ))
            .when_some(error.clone(), |d, e| {
                d.child(div().mt(px(6.)).text_color(p.over).child(e))
            })
            .when(*busy, |d| {
                d.child(div().mt(px(6.)).text_color(p.muted).child("Renaming…"))
            })
            .when(!follows, |d| {
                d.child(
                    div()
                        .id("sub-own-name")
                        .mt(px(8.))
                        .cursor_pointer()
                        .text_color(p.accent)
                        .hover(|s| s.underline())
                        .child("Use the blyg's own name")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.close_reading_sheet(window, cx);
                            this.follow_source_name(id.clone(), window, cx);
                        })),
                )
            })
            .child(self.keys_row(vec![
                self.key_hint("⏎", "rename"),
                self.key_hint("esc", "cancel"),
            ]))
            .into_any_element()
    }

    // ------------------------------------------------------------ folder sheet

    /// New Folder… (or Rename… when `rename` is a folder id).
    pub(crate) fn open_folder_sheet(
        &mut self,
        rename: Option<String>,
        then_file: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = rename
            .as_ref()
            .and_then(|id| self.reading.folders.iter().find(|f| &f.id == id))
            .map(|f| f.name.clone())
            .unwrap_or_default();
        let input = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("Folder name");
            s.set_value(current, window, cx);
            s
        });
        let sub = cx.subscribe_in(&input, window, |this, _, ev, window, cx| match ev {
            InputEvent::PressEnter { .. } => this.commit_folder_sheet(window, cx),
            // Editing the name clears the error (⏎ also reports a change
            // with the text as it was: that keeps it).
            InputEvent::Change => {
                if let Some(RSheet::Folder {
                    input,
                    error,
                    tried,
                    ..
                }) = this.reading.sheet.as_mut()
                    && input.read(cx).value() != tried.as_str()
                {
                    *error = None;
                    cx.notify();
                }
            }
            _ => {}
        });
        self._subs.push(sub);
        input.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        self.open_reading_sheet(
            RSheet::Folder {
                input,
                rename,
                then_file,
                error: None,
                tried: String::new(),
            },
            cx,
        );
    }

    /// ⏎ in the folder sheet: make (or rename) the folder.
    pub(crate) fn commit_folder_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(RSheet::Folder {
            input,
            rename,
            then_file,
            ..
        }) = self.reading.sheet.as_ref()
        else {
            return;
        };
        let name = input.read(cx).value().to_string();
        let (rename, then_file) = (rename.clone(), then_file.clone());
        let r = match &rename {
            Some(id) => self.backend.rename_folder(id, &name).map(|()| None),
            None => self.backend.create_folder(&name).map(Some),
        };
        match r {
            Ok(made) => {
                self.close_reading_sheet(window, cx);
                self.reload_folders(cx);
                if let (Some(f), Some(sub)) = (made.as_ref(), then_file) {
                    self.file_subscription(&sub, Some(&f.id), cx);
                } else if let Some(f) = made {
                    self.show_toast(
                        format!("New folder “{}”", f.name),
                        Some("Drag subscriptions onto it to file them".into()),
                        cx,
                    );
                }
            }
            Err(e) => {
                let msg = match e {
                    blyg_core::CoreError::Rejected { message, .. } => message,
                    e => e.to_string(),
                };
                if let Some(RSheet::Folder { error, tried, .. }) = self.reading.sheet.as_mut() {
                    *error = Some(msg);
                    *tried = name;
                }
                cx.notify();
            }
        }
    }

    pub(super) fn render_folder_sheet(&self, sheet: &RSheet, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette.on_page();
        let RSheet::Folder {
            input,
            rename,
            error,
            ..
        } = sheet
        else {
            return div().into_any_element();
        };
        div()
            .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                cx.stop_propagation();
                this.close_reading_sheet(window, cx);
            }))
            .child(self.sheet_heading(if rename.is_some() {
                "Rename Folder"
            } else {
                "New Folder"
            }))
            .child(div().mb(px(8.)).text_color(p.muted).child(
                "Folders are on this Mac only: they group your subscriptions in the Reader.",
            ))
            .child(self.input_box(
                gpui_kit::base::input::Input::new(input).into_any_element(),
                error.is_some(),
            ))
            .when_some(error.clone(), |d, e| {
                d.child(div().mt(px(6.)).text_color(p.over).child(e))
            })
            .child(self.keys_row(vec![
                self.key_hint("⏎", if rename.is_some() { "rename" } else { "create" }),
                self.key_hint("esc", "cancel"),
            ]))
            .into_any_element()
    }

    // ------------------------------------------------------------ keys

    /// Reader-mode keys; `true` when handled. The list's own ↑/↓ and esc
    /// fall through to `reading_key_down`.
    pub(super) fn reader_key(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let pane = self.reading.pane;
        match key {
            "left" | "right" => {
                let right = key == "right";
                if right && pane != Pane::Post && self.reading.opened.is_none() {
                    // → into a list with nothing open opens the first post,
                    // as NetNewsWire does.
                    self.move_reading(1, window, cx);
                }
                let next = sources_vm::pane_step(
                    pane,
                    right,
                    self.reading.sources_open,
                    self.reading.opened.is_some(),
                );
                self.set_pane(next, cx);
                true
            }
            "enter" if pane == Pane::Sources => {
                if self.reading.opened.is_none() {
                    self.move_reading(1, window, cx);
                }
                self.set_pane(Pane::List, cx);
                true
            }
            "enter" if pane == Pane::List && self.reading.opened.is_some() => {
                self.set_pane(Pane::Post, cx);
                true
            }
            "up" | "down" if pane == Pane::Sources => {
                let delta = if key == "down" { 1 } else { -1 };
                if let Some(s) =
                    sources_vm::step_source(&self.source_entries(), &self.reading.source, delta)
                {
                    self.select_source(s, cx);
                }
                true
            }
            "up" | "down" if pane == Pane::Post => {
                if !self.reader_eval(&crate::app::studio::webview::nudge_js(key == "down")) {
                    self.move_reading(if key == "down" { 1 } else { -1 }, window, cx);
                }
                true
            }
            // j/k: the next / previous post, whichever pane has the keys.
            "j" | "k" if pane != Pane::List => {
                self.move_reading(if key == "j" { 1 } else { -1 }, window, cx);
                true
            }
            "space" => {
                self.reader_space(window, cx);
                true
            }
            "escape" if pane != Pane::List => {
                self.set_pane(Pane::List, cx);
                true
            }
            _ => false,
        }
    }

    pub(crate) fn set_pane(&mut self, pane: Pane, cx: &mut Context<Self>) {
        if self.reading.pane != pane {
            self.reading.pane = pane;
            cx.notify();
        }
    }

    /// Space: page down through the open post; at its end (or with nothing
    /// open), the next post to read.
    fn reader_space(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.reading.opened.is_some()
            && self.reader_eval(crate::app::studio::webview::PAGE_DOWN_JS)
        {
            // The page answers `end` (`SurfaceEvent::PageEnd`) when it was
            // already at the bottom.
            return;
        }
        self.open_next_unread(window, cx);
    }

    /// The next post that needs reading after the open one, in the list.
    pub(crate) fn open_next_unread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.reading.view != View::Reading {
            return;
        }
        let cur = self
            .reading
            .sel
            .as_ref()
            .and_then(|k| self.reading.shown_pos(k));
        match sources_vm::next_unread(&self.reading.rows, &self.reading.shown, cur) {
            Some(pos) => {
                let Some(key) = self.reading.shown_rows().nth(pos).map(super::vm::key) else {
                    return;
                };
                self.reading
                    .list_scroll
                    .scroll_to_item(pos, ScrollStrategy::Nearest);
                if self.reading.mode == ReadMode::Stream {
                    self.reading.stream.list.scroll_to_reveal_item(pos);
                }
                self.open_reading(key, window, cx);
            }
            None => self.show_toast("Nothing else to read here", None, cx),
        }
    }

    // ------------------------------------------------------------ render

    /// The three panes' row: sources | list | post.
    pub(super) fn render_three_panes(
        &self,
        list: AnyElement,
        detail: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette.on_page();
        let open = self.reading.sources_open;
        let post_focus = self.reading.pane == Pane::Post;
        div()
            .id("reader-panes")
            .flex_1()
            .min_h_0()
            .flex()
            .on_drag_move::<DraggedEdge>(cx.listener(
                |this, e: &DragMoveEvent<DraggedEdge>, _, cx| {
                    let w = (e.event.position.x - e.bounds.left()).as_f32();
                    this.reading.sources_w = w.clamp(MIN_W, MAX_W);
                    cx.notify();
                },
            ))
            .on_drop(cx.listener(|this, _: &DraggedEdge, _, cx| {
                let w = this.reading.sources_w.round() as u32;
                if let Some(d) = this.sources_data_dir(cx) {
                    let _ = blyg_core::state::AppState::update(&d, |s| {
                        s.reader_sources_width = Some(w)
                    });
                }
            }))
            .when(open, |d| d.child(self.render_sources_pane(cx)))
            .when(open, |d| {
                d.child(
                    div()
                        .id("sources-edge")
                        .w(px(5.))
                        .ml(px(-3.))
                        .mr(px(-2.))
                        .h_full()
                        .flex_none()
                        .cursor_col_resize()
                        .on_drag(DraggedEdge, |_, _, _, cx| cx.new(|_| DraggedEdge)),
                )
            })
            .child(list)
            .child(
                div()
                    .id("reader-post-pane")
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .relative()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            if this.reading.opened.is_some() {
                                this.set_pane(Pane::Post, cx)
                            }
                        }),
                    )
                    .child(detail)
                    // A thin accent line: this pane has the keys.
                    .when(post_focus, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .right_0()
                                .h(px(2.))
                                .bg(p.accent.opacity(0.6)),
                        )
                    }),
            )
            .children(self.render_source_menu(cx))
            .into_any_element()
    }

    /// The header's sources-pane button (⌥⌘S).
    pub(super) fn render_sources_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette.on_page();
        let on = self.reading.sources_open;
        div()
            .id("sources-toggle")
            .debug_selector(|| "sources-toggle".into())
            .p(px(4.))
            .rounded(px(self.theme.corner(5.)))
            .cursor_pointer()
            .when(on, |d| d.bg(p.well()))
            .hover(|s| s.bg(p.well()))
            .when_some(crate::app::toolbar::icon_svg("panel-left"), |d, bytes| {
                d.child(svg().data(bytes).size(px(14.)).text_color(if on {
                    p.ink
                } else {
                    p.muted
                }))
            })
            .tooltip(move |_, cx| {
                cx.new(|_| {
                    super::Tip(format!(
                        "{} the sources pane · ⌥⌘S",
                        if on { "Hide" } else { "Show" }
                    ))
                })
                .into()
            })
            .on_click(cx.listener(|this, _, window, cx| this.toggle_sources(window, cx)))
            .into_any_element()
    }

    /// The header line above the list: the selected source's name.
    pub(super) fn render_source_title(&self) -> AnyElement {
        let p = self.palette.on_page();
        let label = sources_vm::source_label(
            &self.reading.source,
            &self.reading.folders,
            &self.reading.subs,
        );
        div()
            .id("list-source")
            .flex_none()
            .px(px(14.))
            .pt(px(8.))
            .pb(px(2.))
            .font_family(self.chrome())
            .text_size(px(12.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(p.ink)
            .truncate()
            .child(label)
            .into_any_element()
    }

    fn render_sources_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        // --- themes --- a sidebar: its own ground and ink.
        let p = self.palette.on_side();
        let focused = self.reading.pane == Pane::Sources;
        let rows: Vec<AnyElement> = self
            .source_entries()
            .into_iter()
            .enumerate()
            .map(|(i, e)| self.render_source_entry(i, e, focused, cx))
            .collect();
        div()
            .id("sources-pane")
            .debug_selector(|| "sources-pane".into())
            .w(px(self.reading.sources_w))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .relative()
            .overflow_hidden()
            .bg(p.bg)
            .text_color(p.ink)
            .rule_r(&p)
            .font_family(self.prefs.ui().family)
            .text_size(px(12.5))
            // --- themes --- the sidebar's ground (strata, moss, waves…)
            // behind the rows, as on the posts list.
            .children(crate::ornament::sidebar_ground(&self.theme))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, e: &MouseDownEvent, _, cx| {
                    this.open_source_menu(MenuTarget::Pane, e.position, false, cx)
                }),
            )
            .child(
                div()
                    .id("sources-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .pb(px(12.))
                    .children(rows),
            )
            .children(crate::ornament::scroll_edge(&self.theme))
            .into_any_element()
    }

    fn render_source_entry(
        &self,
        i: usize,
        e: Entry,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette.on_side();
        let selected = e.source().is_some_and(|s| s == self.reading.source);
        // Focused, the row is the theme's `row.selected` (the plain look:
        // an accent wash on the sidebar, with sidebar ink); otherwise it's
        // the selected-row surface, with that row's ink.
        let shaped = self.theme.has(blyg_core::config::theme::Slot::RowSelected);
        let wash = focused && !shaped;
        let ps = if wash { p } else { self.palette.on_sel() };
        let sel_bg = if wash { p.accent.opacity(0.16) } else { ps.bg };
        let theme = self.theme.clone();
        let count = |n: usize| {
            // A private, reader-local number, muted (never social).
            div()
                .flex_none()
                .text_size(px(11.))
                .text_color(p.muted)
                .when(n > 0, |d| d.child(n.to_string()))
        };
        let row = |id: SharedString| {
            div()
                .id(id)
                .mx(px(6.))
                .px(px(8.))
                .h(px(24.))
                .flex()
                .items_center()
                .gap(px(6.))
                .rounded(px(theme.radius.min(6.)))
                .cursor_pointer()
                .when(selected, |d| d.bg(sel_bg).text_color(ps.ink))
                .when(selected && focused && shaped, |d| {
                    crate::ornament::selected_surface(&theme, d, sel_bg)
                })
                .when(!selected, |d| d.hover(|s| s.bg(p.sel.opacity(0.6))))
        };
        let icon = |bytes: &'static [u8]| {
            svg()
                .data(bytes)
                .size(px(14.))
                .flex_none()
                .text_color(if selected { ps.accent } else { p.muted })
        };
        match e {
            Entry::Heading(h) => {
                let unfile = h == "Subscriptions";
                div()
                    .id(("src-heading", i))
                    .px(px(14.))
                    .pt(px(12.))
                    .pb(px(3.))
                    .text_size(px(10.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.muted)
                    .child(h.to_uppercase())
                    // Dropping a subscription on "Subscriptions" unfiles it.
                    .when(unfile, |d| {
                        d.drag_over::<DraggedSub>(move |s, _, _, _| s.text_color(p.accent))
                            .on_drop(cx.listener(|this, d: &DraggedSub, _, cx| {
                                this.file_subscription(&d.id, None, cx)
                            }))
                    })
                    .into_any_element()
            }
            Entry::Smart { smart, unread } => {
                let src = Source::Smart(smart);
                row(SharedString::from(format!("src-smart-{i}")))
                    .debug_selector(move || format!("src-smart-{}", smart.label()))
                    .child(icon(smart_icon(smart)))
                    .child(div().flex_1().min_w_0().truncate().child(smart.label()))
                    .child(count(if smart == Smart::All { 0 } else { unread }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.focus(&this.reading.focus, cx);
                        this.set_pane(Pane::Sources, cx);
                        this.select_source(src.clone(), cx);
                    }))
                    .into_any_element()
            }
            Entry::Folder {
                folder,
                unread,
                expanded,
                ..
            } => {
                let (id1, id2, id3, id4) = (
                    folder.id.clone(),
                    folder.id.clone(),
                    folder.id.clone(),
                    folder.id.clone(),
                );
                let id5 = folder.id.clone();
                let drag = DraggedFolder {
                    id: folder.id.clone(),
                    name: folder.name.clone(),
                };
                let index = folder.position as usize;
                let hl = p.accent.opacity(0.22);
                row(SharedString::from(format!("src-folder-{}", folder.id)))
                    .child(
                        div()
                            .id(SharedString::from(format!("src-disclose-{}", folder.id)))
                            .w(px(10.))
                            .flex_none()
                            .text_size(px(9.))
                            .text_color(p.muted)
                            .child(if expanded { "▾" } else { "▸" })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                if !this.reading.collapsed.remove(&id4) {
                                    this.reading.collapsed.insert(id4.clone());
                                }
                                cx.notify();
                            })),
                    )
                    .child(icon(ICON_FOLDER))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .child(folder.name.clone()),
                    )
                    .child(count(unread))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.focus(&this.reading.focus, cx);
                        this.set_pane(Pane::Sources, cx);
                        this.select_source(Source::Folder(id1.clone()), cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.open_source_menu(
                                MenuTarget::Folder(id2.clone()),
                                e.position,
                                false,
                                cx,
                            )
                        }),
                    )
                    .on_drag(drag, |d, _, _, cx| {
                        let name = d.name.clone();
                        cx.new(|_| DragChip(name))
                    })
                    .drag_over::<DraggedSub>(move |s, _, _, _| s.bg(hl))
                    .drag_over::<DraggedFolder>(move |s, _, _, _| {
                        s.border_t_2().border_color(p.accent)
                    })
                    .on_drop(cx.listener(move |this, d: &DraggedSub, _, cx| {
                        this.file_subscription(&d.id, Some(&id3), cx)
                    }))
                    .on_drop(cx.listener(move |this, d: &DraggedFolder, _, cx| {
                        if d.id != id5 {
                            this.reorder_folder(&d.id, index, cx)
                        }
                    }))
                    .into_any_element()
            }
            Entry::Sub { sub, filed, unread } => {
                let (id1, id2) = (sub.id.clone(), sub.id.clone());
                let name = sources_vm::sub_name(&sub);
                let drag = DraggedSub {
                    id: sub.id.clone(),
                    name: name.clone(),
                };
                // Dropping another subscription here files it beside this one.
                let here = self.reading.filed.get(&sub.id).cloned();
                let hl = p.accent.opacity(0.22);
                row(SharedString::from(format!("src-sub-{}", sub.id)))
                    .when(filed, |d| d.pl(px(26.)))
                    .child(self.source_avatar(&sub))
                    .child(div().flex_1().min_w_0().truncate().child(name.clone()))
                    .when(unread, |d| {
                        d.child(div().flex_none().child(self.unread_marker(p.accent)))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.focus(&this.reading.focus, cx);
                        this.set_pane(Pane::Sources, cx);
                        this.select_source(Source::Sub(id1.clone()), cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.open_source_menu(
                                MenuTarget::Sub(id2.clone()),
                                e.position,
                                false,
                                cx,
                            )
                        }),
                    )
                    .on_drag(drag, |d, _, _, cx| {
                        let name = d.name.clone();
                        cx.new(|_| DragChip(name))
                    })
                    .drag_over::<DraggedSub>(move |s, _, _, _| s.bg(hl))
                    .on_drop(cx.listener(move |this, d: &DraggedSub, _, cx| {
                        this.file_subscription(&d.id, here.as_deref(), cx)
                    }))
                    .into_any_element()
            }
        }
    }

    /// A subscription's avatar when its profile's image is already cached,
    /// else a letter badge.
    fn source_avatar(&self, sub: &blyg_core::Subscription) -> AnyElement {
        let circle = div()
            .size(px(16.))
            .flex_none()
            .rounded_full()
            .overflow_hidden();
        let cached = self
            .reading
            .avatars
            .iter()
            .find(|(o, _)| blyg_core::profile::same_origin(o, &sub.origin))
            .and_then(|(_, url)| crate::images::cached(url));
        if let Some(path) = cached {
            return circle
                .child(
                    img(ImageSource::Resource(Resource::Path(path.into())))
                        .size_full()
                        .object_fit(ObjectFit::Cover),
                )
                .into_any_element();
        }
        // --- themes --- a letter disc in the sidebar's own ink.
        let p = self.palette.on_side();
        circle
            .bg(crate::theme::mix(p.bg, p.ink, 0.08))
            .border_1()
            .border_color(p.muted.opacity(0.5))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(9.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(p.muted)
            .child(sources_vm::letter(&sub.title, &sub.origin))
            .into_any_element()
    }

    /// The context menu, when one is open (drawn above everything).
    pub(crate) fn render_source_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let m = self.reading.src_menu.as_ref()?;
        let p = self.palette.on_page();
        let items = self
            .menu_items()
            .into_iter()
            .enumerate()
            .map(|(i, (label, cmd, checked))| {
                let Some(cmd) = cmd else {
                    return div().my(px(4.)).h(px(1.)).bg(p.line).into_any_element();
                };
                div()
                    .id(("src-menu-item", i))
                    .debug_selector(move || format!("src-menu-{i}"))
                    .px(px(10.))
                    .py(px(4.))
                    .rounded(px(self.theme.corner(5.)))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .cursor_pointer()
                    .hover(|s| s.bg(p.accent).text_color(p.bg))
                    .child(
                        div()
                            .w(px(10.))
                            .flex_none()
                            .child(if checked { "✓" } else { "" }),
                    )
                    .child(label)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.run_menu_cmd(cmd.clone(), window, cx)
                    }))
                    .into_any_element()
            });
        let menu = div()
            .id("src-menu")
            .occlude()
            .min_w(px(170.))
            .p(px(4.))
            .rounded(px(self.theme.radius))
            .border_1()
            .border_color(p.line)
            .bg(p.bg)
            .shadow(vec![BoxShadow {
                color: p.shadow,
                offset: point(px(0.), px(8.)),
                blur_radius: px(24.),
                spread_radius: px(-6.),
                inset: false,
            }])
            .font_family(self.prefs.ui().family)
            .text_size(px(12.5))
            .text_color(p.ink)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.reading.src_menu = None;
                cx.notify();
            }))
            .children(items);
        Some(
            deferred(
                anchored()
                    .position(m.at)
                    .snap_to_window_with_margin(px(8.))
                    .child(menu),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}

/// --- subscription names --- The menu item that hands a renamed
/// subscription's name back to its source.
fn own_name_label(kind: Option<blyg_core::SubscriptionKind>) -> &'static str {
    match kind {
        Some(blyg_core::SubscriptionKind::Rss) => "Use the Feed's Own Name",
        _ => "Use the Blyg's Own Name",
    }
}
