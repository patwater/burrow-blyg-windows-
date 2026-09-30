//! The ⌘G palette: what ⌘G offers when the caret isn't inside a TK scope.
//! Every generating helper is a TK under the hood (disclosed); proofread
//! isn't generated prose and isn't disclosed.

use blyg_core::Kind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Insert an empty `[TK][/TK]` at the caret, caret inside.
    NewGap,
    Shorten,
    Continue,
    Outline,
    Proofread,
    Reply,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub action: Action,
    pub label: &'static str,
    pub detail: &'static str,
    /// The shortcut, shown on the right.
    pub key: Option<&'static str>,
    /// `Some(why)` when it can't run here.
    pub disabled: Option<&'static str>,
}

impl Entry {
    pub fn enabled(&self) -> bool {
        self.disabled.is_none()
    }
}

/// Why "Reply to a reading item" is off.
pub const REPLY_NEEDS_READING: &str = "Open a post in Reading first";

/// The palette for an item of `kind` (fragments get "shorten" and "outline").
/// `reading_open`: a post is open in Reading, so "Reply to a reading item"
/// can answer it.
pub fn entries(kind: Kind, empty: bool, reading_open: bool) -> Vec<Entry> {
    let fragment = kind == Kind::Fragment;
    vec![
        Entry {
            action: Action::NewGap,
            label: "Fill a gap here",
            detail: "Inserts [TK][/TK]: type an instruction, then ⌘G",
            key: None,
            disabled: None,
        },
        Entry {
            action: Action::Shorten,
            label: "Shorten to fit 1000",
            detail: "Proposes a shorter version; you accept or reject it",
            key: Some("⇧⌘G"),
            disabled: (!fragment)
                .then_some("Threads have no length limit")
                .or(empty.then_some("Nothing to shorten yet")),
        },
        Entry {
            action: Action::Continue,
            label: "Continue this thought",
            detail: "Writes the next few sentences at the caret",
            key: None,
            disabled: empty.then_some("Write a few words first"),
        },
        Entry {
            action: Action::Outline,
            label: "Outline a thread",
            detail: "Turns this fragment into a new thread skeleton",
            key: None,
            disabled: empty.then_some("Write a few words first"),
        },
        Entry {
            action: Action::Proofread,
            label: "Proofread",
            detail: "Typos and grammar only, as suggestions · not disclosed",
            key: None,
            disabled: empty.then_some("Nothing to proofread yet"),
        },
        Entry {
            action: Action::Reply,
            label: "Reply to a reading item",
            detail: "Drafts a stub reply to a post you're reading",
            key: None,
            disabled: (!reading_open).then_some(REPLY_NEEDS_READING),
        },
    ]
}

/// Move the selection by `delta`, skipping disabled rows, wrapping.
pub fn step(entries: &[Entry], from: usize, delta: isize) -> usize {
    let n = entries.len() as isize;
    if n == 0 {
        return 0;
    }
    let mut i = from as isize;
    for _ in 0..n {
        i = (i + delta).rem_euclid(n);
        if entries[i as usize].enabled() {
            return i as usize;
        }
    }
    from
}

pub fn first_enabled(entries: &[Entry]) -> usize {
    entries.iter().position(Entry::enabled).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_cant_shorten_and_reply_needs_a_reading_item() {
        let e = entries(Kind::Thread, false, false);
        let get = |a| e.iter().find(|x| x.action == a).unwrap();
        assert!(!get(Action::Shorten).enabled());
        assert!(get(Action::Continue).enabled());
        assert!(!get(Action::Reply).enabled());
        assert!(get(Action::Proofread).detail.contains("not disclosed"));
        let e = entries(Kind::Thread, false, true);
        assert!(
            e.iter()
                .find(|x| x.action == Action::Reply)
                .unwrap()
                .enabled()
        );
    }

    #[test]
    fn stepping_skips_disabled_rows() {
        let e = entries(Kind::Thread, false, false);
        assert_eq!(first_enabled(&e), 0);
        assert_eq!(step(&e, 0, 1), 2, "shorten is skipped");
        assert_eq!(step(&e, 4, 1), 0, "reply is skipped, wraps");
        assert_eq!(step(&e, 0, -1), 4);
    }
}
