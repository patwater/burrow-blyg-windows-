//! --- read/unread --- Picking several posts in the reading list (pure
//! model, no GPUI): ⌘-click adds or drops one, ⇧-click adds the run from
//! the anchor, ⌘A takes everything the list shows, esc lets go. What's
//! picked is what Mark Read / Mark Unread (r / u, the row menu) act on;
//! with nothing picked they act on the open post.

use std::collections::HashSet;

use super::vm::Key;

/// The picked posts and the anchor a ⇧-click runs from.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Picked {
    pub set: HashSet<Key>,
    pub anchor: Option<Key>,
}

impl Picked {
    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    pub fn contains(&self, k: &Key) -> bool {
        self.set.contains(k)
    }

    pub fn clear(&mut self) {
        self.set.clear();
        self.anchor = None;
    }

    /// ⌘-click on `key`. The open post (`current`) joins the first pick, as
    /// a list's selection does.
    pub fn toggle(&mut self, key: &Key, current: Option<&Key>) {
        if self.set.is_empty()
            && let Some(c) = current.filter(|c| *c != key)
        {
            self.set.insert(c.clone());
        }
        if !self.set.remove(key) {
            self.set.insert(key.clone());
        }
        self.anchor = Some(key.clone());
    }

    /// ⇧-click on `key`: everything between the anchor (else the open
    /// post, else `key` itself) and `key`, in the list's order (`shown`).
    pub fn extend(&mut self, key: &Key, current: Option<&Key>, shown: &[Key]) {
        let from = self.anchor.as_ref().or(current).unwrap_or(key);
        let (Some(a), Some(b)) = (
            shown.iter().position(|k| k == from),
            shown.iter().position(|k| k == key),
        ) else {
            self.set.insert(key.clone());
            return;
        };
        let (lo, hi) = (a.min(b), a.max(b));
        self.set.extend(shown[lo..=hi].iter().cloned());
        if self.anchor.is_none() {
            self.anchor = Some(from.clone());
        }
    }

    /// ⌘A: everything the list shows.
    pub fn all(&mut self, shown: &[Key]) {
        self.set = shown.iter().cloned().collect();
        self.anchor = shown.first().cloned();
    }

    /// Drop what the list no longer shows (a new source or search).
    pub fn retain_shown(&mut self, shown: &[Key]) {
        let keep: HashSet<&Key> = shown.iter().collect();
        self.set.retain(|k| keep.contains(k));
        if self.anchor.as_ref().is_some_and(|a| !keep.contains(a)) {
            self.anchor = None;
        }
    }

    /// What a mark acts on, in the list's order: the picked posts, else
    /// `fallback` (the open post).
    pub fn targets(&self, shown: &[Key], fallback: Option<&Key>) -> Vec<Key> {
        if self.set.is_empty() {
            return fallback.cloned().into_iter().collect();
        }
        let mut v: Vec<Key> = shown
            .iter()
            .filter(|k| self.set.contains(*k))
            .cloned()
            .collect();
        // Picked rows the list no longer shows still count.
        let mut rest: Vec<Key> = self
            .set
            .iter()
            .filter(|k| !v.contains(k))
            .cloned()
            .collect();
        rest.sort();
        v.extend(rest);
        v
    }
}

/// The label of a mark over `n` posts.
pub fn mark_label(read: bool, n: usize) -> String {
    let what = if read { "Read" } else { "Unread" };
    if n <= 1 {
        format!("Mark {what}")
    } else {
        format!("Mark {n} {what}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(s: &str) -> Key {
        ("sub".to_string(), s.to_string())
    }

    fn keys(v: &[&str]) -> Vec<Key> {
        v.iter().map(|s| k(s)).collect()
    }

    #[test]
    fn cmd_click_adds_and_drops_one_with_the_open_post_joining() {
        let mut p = Picked::default();
        p.toggle(&k("c"), Some(&k("a")));
        assert_eq!(p.set.len(), 2, "the open post joins the first pick");
        p.toggle(&k("a"), Some(&k("a")));
        assert_eq!(p.set, [k("c")].into_iter().collect());
        p.toggle(&k("c"), None);
        assert!(p.is_empty());
    }

    #[test]
    fn shift_click_takes_the_run_from_the_anchor() {
        let shown = keys(&["a", "b", "c", "d", "e"]);
        let mut p = Picked::default();
        p.toggle(&k("b"), None);
        p.extend(&k("d"), None, &shown);
        assert_eq!(p.targets(&shown, None), keys(&["b", "c", "d"]));
        // Upwards from the open post when nothing is anchored.
        let mut p = Picked::default();
        p.extend(&k("a"), Some(&k("c")), &shown);
        assert_eq!(p.targets(&shown, None), keys(&["a", "b", "c"]));
    }

    #[test]
    fn all_then_esc() {
        let shown = keys(&["a", "b", "c"]);
        let mut p = Picked::default();
        p.all(&shown);
        assert_eq!(p.targets(&shown, None), shown);
        p.clear();
        assert!(p.is_empty());
        assert_eq!(
            p.targets(&shown, Some(&k("b"))),
            keys(&["b"]),
            "the open post"
        );
        assert!(p.targets(&shown, None).is_empty());
    }

    #[test]
    fn a_new_view_drops_what_it_doesnt_show() {
        let mut p = Picked::default();
        p.all(&keys(&["a", "b", "c"]));
        p.retain_shown(&keys(&["b", "z"]));
        assert_eq!(p.set, [k("b")].into_iter().collect());
        assert_eq!(p.anchor, None);
    }

    #[test]
    fn labels() {
        assert_eq!(mark_label(true, 1), "Mark Read");
        assert_eq!(mark_label(false, 3), "Mark 3 Unread");
    }
}
