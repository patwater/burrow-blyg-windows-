//! The tutorial's script: one row per step, in order. Edit the table to
//! change the tour; `tutorial.rs` only interprets it.
//!
//! A step waits for one of its `keys` (a GPUI action the user triggers in
//! the real window, never synthetic input), then moves on. Next and Back
//! always work too. `setup` prepares the sample data for the step when it
//! is entered, from either direction.

/// Something the user does that a step can wait for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// ↑ or ↓ in the omnibar.
    MoveSelection,
    /// ⏎ (the omnibar, or a sheet's field).
    Enter,
    /// Any edit in the editor (autosave).
    Typed,
    ToggleKind,
    MakeDraft,
    Publish,
    ViewWrite,
    ViewSplit,
    ViewStudio,
    AiGenerate,
    AiShorten,
    ShowVersions,
    ShowReading,
    QuotePicker,
}

/// The part of the window a step highlights.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    Omnibar,
    /// The omnibar and the list together.
    Search,
    List,
    Editor,
    /// The status bar's left end: ◦ fragment / ≡ thread and the counter.
    Counter,
    /// The Posts · Reading · … switcher in the title bar.
    ViewSwitcher,
    /// A sheet dropping from the title bar.
    Sheet,
    /// The whole window (no ring).
    Whole,
}

/// What a step prepares on the sample data when it's entered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setup {
    Nothing,
    /// Posts, ⌘1, an empty omnibar with the focus in it.
    Search,
    /// A query with no match in the omnibar, caret at the end.
    Query(&'static str),
    /// Make sure a post is open in the editor.
    EnsureOpen,
    /// A fresh scratch note (as quick capture would keep it), opened.
    Scratch(&'static str),
    /// A draft with something to publish, opened.
    Publishable,
    /// The publish sheet, if it isn't already open.
    PublishSheet,
    /// The sample thread (a quote, a TK and a video), opened in ⌘1.
    StudioSample,
    /// The draft with a TK scope appended, caret inside it.
    Tk(&'static str),
    /// The draft pushed over 1000 characters.
    Long(&'static str),
    /// A published post with a few versions, opened.
    Versioned,
    /// Back to Posts.
    Posts,
    /// A thread draft opened (quotes go in threads).
    Thread,
}

/// One step of the tour.
#[derive(Debug, Clone, Copy)]
pub struct Step {
    pub id: &'static str,
    pub title: &'static str,
    /// One or two short sentences. `{hotkey}` becomes the quick-capture key.
    pub caption: &'static str,
    /// Any of these moves on. Empty: only Next does.
    pub keys: &'static [Key],
    /// How the key is shown on the card, e.g. "⌘T".
    pub key_label: &'static str,
    pub region: Region,
    /// The matching toolbar button's label, pointed at when buttons are on.
    pub button: Option<&'static str>,
    pub setup: Setup,
    /// After the key: how long the card shows "✓" before moving on.
    pub pause_ms: u64,
    /// Also wait until sheets, AI proposals and generations are done
    /// (so the user sees what the key did).
    pub settle: bool,
}

impl Step {
    pub fn accepts(&self, key: Key) -> bool {
        self.keys.contains(&key)
    }
}

/// Sample post ids in the tutorial's FakeBackend.
pub const DRAFT: &str = "01J9QK3";
pub const VERSIONED: &str = "01J9M2A";
pub const THREAD: &str = "01J9H4C";

pub const STEPS: &[Step] = &[
    Step {
        id: "search",
        title: "Search is the interface",
        caption: "Type to filter every post as you go. ↑ and ↓ move the selection, and the \
                  editor previews each post as you pass it.",
        keys: &[Key::MoveSelection],
        key_label: "↑ ↓",
        region: Region::Search,
        button: None,
        setup: Setup::Search,
        pause_ms: 900,
        settle: false,
    },
    Step {
        id: "create",
        title: "No match? ⏎ starts writing",
        caption: "Nothing matches “tide pools”, so ⏎ creates a new draft seeded with it, \
                  with the caret at the end.",
        keys: &[Key::Enter],
        key_label: "⏎",
        region: Region::Omnibar,
        button: Some("New"),
        setup: Setup::Query("tide pools"),
        pause_ms: 500,
        settle: false,
    },
    Step {
        id: "autosave",
        title: "Just write",
        caption: "Every keystroke is saved on this Mac and pushed to your blyg a moment after \
                  you stop. There's no save button. Type a few words.",
        keys: &[Key::Typed],
        key_label: "type",
        region: Region::Editor,
        button: None,
        setup: Setup::EnsureOpen,
        pause_ms: 1600,
        settle: false,
    },
    Step {
        id: "kinds",
        title: "Fragments and threads",
        caption: "A fragment is short: 1000 characters at most, counted down here. A thread \
                  has no limit and can quote other posts. ⌘T switches between them.",
        keys: &[Key::ToggleKind],
        key_label: "⌘T",
        region: Region::Counter,
        button: None,
        setup: Setup::EnsureOpen,
        pause_ms: 1500,
        settle: false,
    },
    Step {
        id: "capture",
        title: "Quick capture keeps scratch notes",
        caption: "{hotkey} opens quick capture over any app. What you jot down stays on this \
                  Mac as a scratch note, never synced. ⌘D makes it a draft; ⌘⏎ publishes it.",
        keys: &[Key::MakeDraft],
        key_label: "⌘D",
        region: Region::List,
        button: Some("Make draft"),
        setup: Setup::Scratch(
            "Tide tables are poems nobody reads aloud: caught with quick capture.",
        ),
        pause_ms: 1400,
        settle: false,
    },
    Step {
        id: "publish",
        title: "⌘⏎ publishes",
        caption: "Publishing asks for an optional version note first. This is sample data, so \
                  nothing leaves your Mac.",
        keys: &[Key::Publish],
        key_label: "⌘⏎",
        region: Region::Editor,
        button: Some("Publish"),
        setup: Setup::Publishable,
        pause_ms: 150,
        settle: false,
    },
    Step {
        id: "publish-note",
        title: "Say what changed",
        caption: "Type a note such as “first version” (or leave it empty) and press ⏎. \
                  esc would cancel.",
        keys: &[Key::Enter, Key::Publish],
        key_label: "⏎",
        region: Region::Sheet,
        button: None,
        setup: Setup::PublishSheet,
        pause_ms: 1200,
        settle: false,
    },
    Step {
        id: "full-editor",
        title: "The full editor",
        caption: "⌘2 adds a live preview beside the list; ⌘3 is the full editor: the source \
                  and the page exactly as it will be published.",
        keys: &[Key::ViewStudio, Key::ViewSplit],
        key_label: "⌘3",
        region: Region::Whole,
        button: Some("Full editor"),
        setup: Setup::StudioSample,
        pause_ms: 2200,
        settle: false,
    },
    Step {
        id: "write-view",
        title: "Back to writing",
        caption: "⌘1 brings back the plain list and editor.",
        keys: &[Key::ViewWrite],
        key_label: "⌘1",
        region: Region::Whole,
        button: Some("Write"),
        setup: Setup::Nothing,
        pause_ms: 600,
        settle: false,
    },
    Step {
        id: "tk",
        title: "Fill a gap with AI",
        caption: "Write [TK]an instruction[/TK] and press ⌘G with the caret inside: the gap is \
                  filled, tinted as generated, and disclosed when published. (A canned demo: \
                  no AI is called.)",
        keys: &[Key::AiGenerate],
        key_label: "⌘G",
        region: Region::Editor,
        button: Some("Generate"),
        setup: Setup::Tk("one short sentence about fog, in my voice"),
        pause_ms: 1200,
        settle: true,
    },
    Step {
        id: "shorten",
        title: "Too long? Shorten to fit",
        caption: "This fragment is over 1000. ⇧⌘G proposes a shorter version; you accept or \
                  reject it. (Canned, too.)",
        keys: &[Key::AiShorten],
        key_label: "⇧⌘G",
        region: Region::Counter,
        button: None,
        setup: Setup::Long(
            "The log records wind, swell and visibility, hour after hour, in the same hand. ",
        ),
        pause_ms: 600,
        settle: true,
    },
    Step {
        id: "versions",
        title: "Versions and pins",
        caption: "⌘Y lists every version of your post; the ‹ vN ▾ › pill steps through them. \
                  Pinning a version serves it forever: a pin can't be undone.",
        keys: &[Key::ShowVersions],
        key_label: "⌘Y",
        region: Region::Editor,
        button: Some("Versions"),
        setup: Setup::Versioned,
        pause_ms: 2800,
        settle: false,
    },
    Step {
        id: "reading",
        title: "Reading",
        caption: "⌘R shows the blygs you follow, one entry per post; / searches them. Under a \
                  post, Reply starts a stub (your thread answering it), Quote puts it in a \
                  thread of yours, and Fork works from a 📌 pinned version. ⌘R again comes \
                  back to your posts.",
        keys: &[Key::ShowReading],
        key_label: "⌘R",
        region: Region::ViewSwitcher,
        button: None,
        setup: Setup::Posts,
        pause_ms: 2400,
        settle: false,
    },
    Step {
        id: "quotes",
        title: "Quotes",
        caption: "In a thread, ⌘K (or typing ![[ on a new line) quotes a post you already \
                  hold. The quote is a snapshot, taken when you publish.",
        keys: &[Key::QuotePicker],
        key_label: "⌘K",
        region: Region::Editor,
        button: None,
        setup: Setup::Thread,
        pause_ms: 800,
        settle: true,
    },
    Step {
        id: "done",
        title: "That's the tour",
        caption: "Everything you did here was on sample data. Your own posts come back when you \
                  finish. Settings (⌘,) › Help replays this tour.",
        keys: &[],
        key_label: "",
        region: Region::Whole,
        button: None,
        setup: Setup::Posts,
        pause_ms: 0,
        settle: false,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_sound() {
        let mut ids: Vec<_> = STEPS.iter().map(|s| s.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), STEPS.len(), "duplicate step id");
        for (i, s) in STEPS.iter().enumerate() {
            assert!(!s.title.is_empty() && !s.caption.is_empty(), "{}", s.id);
            let last = i + 1 == STEPS.len();
            assert_eq!(
                s.keys.is_empty(),
                last,
                "{}: only the last step waits for Next",
                s.id
            );
            assert_eq!(s.key_label.is_empty(), s.keys.is_empty(), "{}", s.id);
        }
        // Every topic in the spec is covered.
        for k in [
            Key::MoveSelection,
            Key::Enter,
            Key::Typed,
            Key::ToggleKind,
            Key::MakeDraft,
            Key::Publish,
            Key::ViewStudio,
            Key::ViewWrite,
            Key::AiGenerate,
            Key::AiShorten,
            Key::ShowVersions,
            Key::ShowReading,
            Key::QuotePicker,
        ] {
            assert!(STEPS.iter().any(|s| s.accepts(k)), "{k:?} isn't taught");
        }
    }
}
