//! --- read/unread --- Picking posts in the reading list and marking them
//! read or unread (model: `pick_vm.rs`). ⌘-click / ⇧-click / ⌘A pick,
//! esc lets go; r marks read, u unread (also the row's menu: right-click).
//! With nothing picked, the marks act on the open post. Marks are local and
//! instant (`Backend::set_read`); a server that keeps read state gets them
//! in one batch, and unread reaches only a server that can clear read
//! state (otherwise it stays on this Mac).

use gpui_kit::*;

use super::pick_vm::{self, Picked};
use super::stream_vm::ReadMode;
use super::vm::{self, Key};
use crate::app::MainView;

impl MainView {
    /// The keys the list shows, in order.
    pub(crate) fn shown_keys(&self) -> Vec<Key> {
        self.reading.shown_rows().map(vm::key).collect()
    }

    /// A row clicked with ⌘ or ⇧ (Reader mode): pick instead of opening.
    /// False for a plain click, which lets go of the picks and opens it.
    pub(crate) fn pick_click(&mut self, key: &Key, m: Modifiers, cx: &mut Context<Self>) -> bool {
        if self.reading.mode != ReadMode::Reader {
            return false;
        }
        let current = self.reading.sel.clone();
        if m.platform && !m.shift {
            self.reading.picked.toggle(key, current.as_ref());
        } else if m.shift {
            let shown = self.shown_keys();
            self.reading.picked.extend(key, current.as_ref(), &shown);
        } else {
            if !self.reading.picked.is_empty() {
                self.reading.picked.clear();
                cx.notify();
            }
            return false;
        }
        cx.notify();
        true
    }

    /// ⌘A in the list: pick everything it shows.
    pub(crate) fn pick_all(&mut self, cx: &mut Context<Self>) {
        let shown = self.shown_keys();
        self.reading.picked.all(&shown);
        if !shown.is_empty() {
            self.show_toast(
                format!("{} picked", shown.len()),
                Some("r marks them read, u unread · esc lets go".into()),
                cx,
            );
        }
        cx.notify();
    }

    /// What a mark acts on now: the picked posts, else the open one.
    pub(crate) fn mark_targets(&self) -> Vec<Key> {
        let open = self
            .reading
            .opened
            .as_ref()
            .map(|o| o.key.clone())
            .or_else(|| self.reading.sel.clone());
        self.reading
            .picked
            .targets(&self.shown_keys(), open.as_ref())
    }

    /// Mark these posts read or unread (local, instant).
    pub(crate) fn mark_posts(&mut self, keys: Vec<Key>, read: bool, cx: &mut Context<Self>) {
        if keys.is_empty() {
            return self.show_toast("Nothing to mark", Some(PICK_HOW.into()), cx);
        }
        let n = keys.len();
        match self.backend.set_read(&keys, read) {
            Ok(_) => {
                // The open post keeps "edited since you read it" from when it
                // opened; a mark read clears that, a mark unread shows it.
                if let Some(o) = self.reading.opened.as_mut()
                    && keys.contains(&o.key)
                {
                    o.item.read_version = (read).then_some(o.item.version);
                }
                self.show_toast(
                    format!(
                        "Marked {} {}",
                        if n == 1 {
                            "1 post".to_string()
                        } else {
                            format!("{n} posts")
                        },
                        if read { "read" } else { "unread" }
                    ),
                    None,
                    cx,
                );
                self.reading_changed(cx);
            }
            Err(e) => self.show_toast(format!("Couldn't mark them: {e}"), None, cx),
        }
    }

    /// r / u, and the row menu's Mark Read / Mark Unread.
    pub(crate) fn mark_picked(&mut self, read: bool, cx: &mut Context<Self>) {
        let keys = self.mark_targets();
        self.mark_posts(keys, read, cx);
    }

    /// Right-click on a row: its menu acts on the picks when it's among
    /// them, else on that row alone (the picks let go).
    pub(crate) fn open_post_menu(&mut self, key: Key, at: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.reading.picked.contains(&key) {
            self.reading.picked = Picked::default();
        }
        self.open_source_menu(super::sources::MenuTarget::Posts(key), at, false, cx);
    }

    /// The row menu's items (`menu_items`).
    pub(crate) fn post_menu_items(&self, key: &Key) -> Vec<(String, super::sources::MenuCmd)> {
        use super::sources::MenuCmd;
        let keys = if self.reading.picked.contains(key) {
            self.reading.picked.targets(&self.shown_keys(), None)
        } else {
            vec![key.clone()]
        };
        let n = keys.len();
        let rows: Vec<_> = keys
            .iter()
            .filter_map(|k| self.reading.rows.iter().find(|r| vm::key(r) == *k))
            .collect();
        let any_read = rows.iter().any(|r| r.read_version.is_some());
        let any_unread = rows.iter().any(|r| r.is_unread() || r.edited_since_read());
        let mut v = Vec::new();
        if any_unread || rows.is_empty() {
            v.push((
                pick_vm::mark_label(true, n),
                MenuCmd::Mark(keys.clone(), true),
            ));
        }
        if any_read || rows.is_empty() {
            v.push((pick_vm::mark_label(false, n), MenuCmd::Mark(keys, false)));
        }
        v
    }
}

pub(crate) const PICK_HOW: &str = "Open a post, or pick some: ⌘-click, ⇧-click, ⌘A";
