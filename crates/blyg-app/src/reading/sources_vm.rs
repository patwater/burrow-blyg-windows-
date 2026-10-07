//! --- reader folders --- Pure rules for the Reader's sources pane: the
//! smart feeds, folders and subscriptions it lists (in order, with their
//! unread state), which posts a selected source shows, and how focus moves
//! between the three panes. `sources.rs` renders what these say.

use std::collections::{HashMap, HashSet};

use blyg_core::{Folder, ReadingItem, Subscription};
use chrono::{DateTime, Local, Utc};

use super::vm::Key;

/// The smart feeds at the top of the sources pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Smart {
    /// Posts you haven't read, or that were edited since you read them.
    Unread,
    /// Posts dated today (the post's own date, local time).
    Today,
    /// Posts you gave a thumb up.
    Thumbed,
    /// Everything held.
    All,
}

impl Smart {
    pub const ALL: [Smart; 4] = [Smart::Unread, Smart::Today, Smart::Thumbed, Smart::All];

    pub fn label(self) -> &'static str {
        match self {
            Smart::Unread => "All unread",
            Smart::Today => "Today",
            Smart::Thumbed => "Thumbed",
            Smart::All => "All",
        }
    }
}

/// What the middle pane lists.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Source {
    Smart(Smart),
    /// A folder id: the posts of every subscription filed in it.
    Folder(String),
    /// A subscription id.
    Sub(String),
}

impl Default for Source {
    fn default() -> Self {
        Source::Smart(Smart::All)
    }
}

/// Which of the three panes has the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pane {
    Sources,
    #[default]
    List,
    Post,
}

/// ← / → from `pane`. ← stops at the sources (or at the list when the
/// sources pane is hidden); → reaches the post only when one is open.
pub fn pane_step(pane: Pane, right: bool, sources_open: bool, post_open: bool) -> Pane {
    match (pane, right) {
        (Pane::Sources, true) => Pane::List,
        (Pane::List, true) if post_open => Pane::Post,
        (Pane::List, false) if sources_open => Pane::Sources,
        (Pane::Post, false) => Pane::List,
        (p, _) => p,
    }
}

/// Counts as "to read" for the unread smart feed, the folder counts and
/// the dots: never read, or edited since you read it.
pub fn needs_reading(r: &ReadingItem) -> bool {
    r.is_unread() || (r.edited_since_read() && r.state != "tombstone")
}

/// The post's own date (updated, else created) falls on today, local time.
pub fn is_today(r: &ReadingItem, now: DateTime<Utc>) -> bool {
    let t = r.post_time();
    if t.observed {
        // No date of its own: never "today" just because it was imported.
        return false;
    }
    DateTime::parse_from_rfc3339(&t.at)
        .map(|d| d.with_timezone(&Local).date_naive() == now.with_timezone(&Local).date_naive())
        .unwrap_or(false)
}

/// Does `r` belong to `source`? `filed` = subscription id → folder id.
pub fn in_source(
    r: &ReadingItem,
    source: &Source,
    filed: &HashMap<String, String>,
    now: DateTime<Utc>,
) -> bool {
    match source {
        Source::Smart(Smart::All) => true,
        Source::Smart(Smart::Unread) => needs_reading(r),
        Source::Smart(Smart::Today) => is_today(r, now),
        Source::Smart(Smart::Thumbed) => r.thumb == Some(1),
        Source::Folder(f) => filed.get(&r.subscription_id) == Some(f),
        Source::Sub(s) => &r.subscription_id == s,
    }
}

/// The rows (indices into `rows`, from the search's `searched`) the list
/// shows for `source`. Rows in `sticky` stay even when they no longer match
/// (a post read in "All unread" doesn't vanish under the cursor).
pub fn filter_source(
    rows: &[ReadingItem],
    searched: &[usize],
    source: &Source,
    filed: &HashMap<String, String>,
    sticky: &HashSet<Key>,
    now: DateTime<Utc>,
) -> Vec<usize> {
    searched
        .iter()
        .copied()
        .filter(|&i| {
            rows.get(i).is_some_and(|r| {
                in_source(r, source, filed, now)
                    || sticky.contains(&(r.subscription_id.clone(), r.remote_id.clone()))
            })
        })
        .collect()
}

/// One line of the sources pane.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    /// "Smart feeds", "Folders", "Subscriptions".
    Heading(&'static str),
    Smart {
        smart: Smart,
        /// Posts to read in it (a private, reader-local number).
        unread: usize,
    },
    Folder {
        folder: Folder,
        unread: usize,
        expanded: bool,
        /// Subscriptions filed in it.
        subs: usize,
    },
    Sub {
        sub: Subscription,
        /// Inside a folder (indented).
        filed: bool,
        /// Anything to read: a dot, never a count.
        unread: bool,
    },
}

impl Entry {
    /// The source this line selects (headings select nothing).
    pub fn source(&self) -> Option<Source> {
        match self {
            Entry::Heading(_) => None,
            Entry::Smart { smart, .. } => Some(Source::Smart(*smart)),
            Entry::Folder { folder, .. } => Some(Source::Folder(folder.id.clone())),
            Entry::Sub { sub, .. } => Some(Source::Sub(sub.id.clone())),
        }
    }
}

/// Everything the sources pane lists, top to bottom: the smart feeds, each
/// folder with its subscriptions (unless collapsed), then the subscriptions
/// in no folder. Subscriptions keep the server's order within a group.
pub fn entries(
    folders: &[Folder],
    filed: &HashMap<String, String>,
    subs: &[Subscription],
    rows: &[ReadingItem],
    collapsed: &HashSet<String>,
    now: DateTime<Utc>,
) -> Vec<Entry> {
    let mut unread_by_sub: HashMap<&str, usize> = HashMap::new();
    for r in rows.iter().filter(|r| needs_reading(r)) {
        *unread_by_sub.entry(r.subscription_id.as_str()).or_default() += 1;
    }
    let unread_of = |pred: &dyn Fn(&ReadingItem) -> bool| {
        rows.iter().filter(|r| needs_reading(r) && pred(r)).count()
    };
    let mut out = vec![Entry::Heading("Smart feeds")];
    for smart in Smart::ALL {
        let src = Source::Smart(smart);
        out.push(Entry::Smart {
            smart,
            unread: unread_of(&|r| in_source(r, &src, filed, now)),
        });
    }
    // A folder id that no longer exists counts as unfiled.
    let known: HashSet<&str> = folders.iter().map(|f| f.id.as_str()).collect();
    let folder_of = |s: &Subscription| {
        filed
            .get(&s.id)
            .map(String::as_str)
            .filter(|f| known.contains(f))
    };
    let sub_entry = |s: &Subscription, in_folder: bool| Entry::Sub {
        sub: s.clone(),
        filed: in_folder,
        unread: unread_by_sub.get(s.id.as_str()).copied().unwrap_or(0) > 0,
    };
    if !folders.is_empty() {
        out.push(Entry::Heading("Folders"));
    }
    for f in folders {
        let members: Vec<&Subscription> = subs
            .iter()
            .filter(|s| folder_of(s) == Some(f.id.as_str()))
            .collect();
        let unread = members
            .iter()
            .map(|s| unread_by_sub.get(s.id.as_str()).copied().unwrap_or(0))
            .sum();
        let expanded = !collapsed.contains(&f.id);
        out.push(Entry::Folder {
            folder: f.clone(),
            unread,
            expanded,
            subs: members.len(),
        });
        if expanded {
            out.extend(members.into_iter().map(|s| sub_entry(s, true)));
        }
    }
    let unfiled: Vec<&Subscription> = subs.iter().filter(|s| folder_of(s).is_none()).collect();
    if !unfiled.is_empty() {
        out.push(Entry::Heading("Subscriptions"));
        out.extend(unfiled.into_iter().map(|s| sub_entry(s, false)));
    }
    out
}

/// The selectable sources in pane order (↑/↓ move through these).
pub fn selectable(entries: &[Entry]) -> Vec<Source> {
    entries.iter().filter_map(Entry::source).collect()
}

/// ↑/↓ in the sources pane: the source `delta` steps from `current`
/// (clamped); the first one when `current` isn't listed.
pub fn step_source(entries: &[Entry], current: &Source, delta: isize) -> Option<Source> {
    let list = selectable(entries);
    if list.is_empty() {
        return None;
    }
    let next = match list.iter().position(|s| s == current) {
        None => 0,
        Some(i) => (i as isize + delta).clamp(0, list.len() as isize - 1) as usize,
    };
    list.get(next).cloned()
}

/// The letter badge for a subscription without a cached avatar.
pub fn letter(title: &str, origin: &str) -> String {
    let src = if title.trim().is_empty() {
        super::vm::host(origin)
    } else {
        title.to_string()
    };
    src.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "·".into())
}

/// The name a subscription row shows: its title, else its host.
pub fn sub_name(s: &Subscription) -> String {
    if s.title.trim().is_empty() {
        super::vm::host(&s.origin)
    } else {
        s.title.clone()
    }
}

/// The source's name, for the list header and the empty state.
pub fn source_label(source: &Source, folders: &[Folder], subs: &[Subscription]) -> String {
    match source {
        Source::Smart(s) => s.label().to_string(),
        Source::Folder(id) => folders
            .iter()
            .find(|f| &f.id == id)
            .map(|f| f.name.clone())
            .unwrap_or_else(|| "Folder".into()),
        Source::Sub(id) => subs
            .iter()
            .find(|s| &s.id == id)
            .map(sub_name)
            .unwrap_or_else(|| "Subscription".into()),
    }
}

/// What the list says when the selected source has nothing to show.
pub fn empty_label(source: &Source) -> &'static str {
    match source {
        Source::Smart(Smart::Unread) => "All read. Nothing new to read.",
        Source::Smart(Smart::Today) => "Nothing dated today.",
        Source::Smart(Smart::Thumbed) => "No thumbed posts yet. 👍 a post to keep it here.",
        Source::Smart(Smart::All) => "Nothing to read yet.",
        Source::Folder(_) => "Nothing here. Drag a subscription onto the folder to file it.",
        Source::Sub(_) => "No posts from this subscription yet.",
    }
}

/// After the current post (position `from` in `shown`, or before the top),
/// the next one that needs reading: Space's "next unread".
pub fn next_unread(rows: &[ReadingItem], shown: &[usize], from: Option<usize>) -> Option<usize> {
    let start = from.map(|i| i + 1).unwrap_or(0);
    (start..shown.len()).find(|&p| rows.get(shown[p]).is_some_and(needs_reading))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::reading_seed::{self, *};

    fn seed() -> (reading_seed::Seed, DateTime<Utc>) {
        let now = Utc::now();
        (reading_seed::seed(now), now)
    }

    #[test]
    fn panes_step_left_and_right() {
        use Pane::*;
        assert_eq!(pane_step(List, false, true, false), Sources);
        assert_eq!(pane_step(List, false, false, false), List, "sources hidden");
        assert_eq!(pane_step(Sources, false, true, true), Sources);
        assert_eq!(pane_step(Sources, true, true, false), List);
        assert_eq!(pane_step(List, true, true, false), List, "nothing open");
        assert_eq!(pane_step(List, true, true, true), Post);
        assert_eq!(pane_step(Post, true, true, true), Post);
        assert_eq!(pane_step(Post, false, true, true), List);
    }

    #[test]
    fn the_pane_lists_smart_feeds_then_folders_then_unfiled() {
        let (sd, now) = seed();
        let rows = super::super::vm::order(sd.reading.clone());
        let e = entries(
            &sd.folders,
            &sd.filed,
            &sd.subs,
            &rows,
            &HashSet::new(),
            now,
        );
        let lines: Vec<String> = e
            .iter()
            .map(|e| match e {
                Entry::Heading(h) => format!("# {h}"),
                Entry::Smart { smart, .. } => smart.label().into(),
                Entry::Folder { folder, .. } => format!("[{}]", folder.name),
                Entry::Sub { sub, filed, .. } => {
                    format!("{}{}", if *filed { "  " } else { "" }, sub.title)
                }
            })
            .collect();
        assert_eq!(
            lines,
            [
                "# Smart feeds",
                "All unread",
                "Today",
                "Thumbed",
                "All",
                "# Folders",
                "[Friends]",
                "  Rue",
                "  Ada",
                "[Gardens]",
                "  Lin",
                "# Subscriptions",
                "Omar's notes",
            ]
        );
        // Collapsing a folder hides its subscriptions, not the folder.
        let e2 = entries(
            &sd.folders,
            &sd.filed,
            &sd.subs,
            &rows,
            &HashSet::from([FOLDER_FRIENDS.to_string()]),
            now,
        );
        assert!(
            !e2.iter()
                .any(|e| matches!(e, Entry::Sub { sub, .. } if sub.id == "sub-rue"))
        );
        assert!(e2.iter().any(
            |e| matches!(e, Entry::Folder { folder, expanded: false, subs: 2, .. } if folder.id == FOLDER_FRIENDS)
        ));
    }

    #[test]
    fn counts_are_unread_posts_and_subscriptions_get_a_dot() {
        let (sd, now) = seed();
        let rows = super::super::vm::order(sd.reading.clone());
        let e = entries(
            &sd.folders,
            &sd.filed,
            &sd.subs,
            &rows,
            &HashSet::new(),
            now,
        );
        let unread_total = rows.iter().filter(|r| needs_reading(r)).count();
        assert!(unread_total > 0);
        assert!(e.iter().any(
            |e| matches!(e, Entry::Smart { smart: Smart::Unread, unread } if *unread == unread_total)
        ));
        // Friends = Rue + Ada.
        let friends = rows
            .iter()
            .filter(|r| {
                needs_reading(r) && ["sub-rue", "sub-ada"].contains(&r.subscription_id.as_str())
            })
            .count();
        assert!(e.iter().any(
            |e| matches!(e, Entry::Folder { folder, unread, .. } if folder.id == FOLDER_FRIENDS && *unread == friends)
        ));
        // Omar's one post was read: no dot.
        assert!(
            e.iter().any(
                |e| matches!(e, Entry::Sub { sub, unread: false, .. } if sub.id == "sub-omar")
            )
        );
    }

    #[test]
    fn sources_filter_the_list() {
        let (sd, now) = seed();
        let rows = super::super::vm::order(sd.reading.clone());
        let all: Vec<usize> = (0..rows.len()).collect();
        let ids = |src: Source, sticky: &HashSet<Key>| -> Vec<String> {
            filter_source(&rows, &all, &src, &sd.filed, sticky, now)
                .into_iter()
                .map(|i| rows[i].remote_id.clone())
                .collect()
        };
        let none = HashSet::new();
        let gardens = ids(Source::Folder(FOLDER_GARDENS.into()), &none);
        assert!(!gardens.is_empty());
        assert!(gardens.iter().all(|id| {
            rows.iter()
                .any(|r| &r.remote_id == id && r.subscription_id == "sub-lin")
        }));
        assert_eq!(ids(Source::Sub("sub-omar".into()), &none), [OMAR_YEAR]);
        assert_eq!(ids(Source::Smart(Smart::Thumbed), &none), [RUE_KEPT]);
        let unread = ids(Source::Smart(Smart::Unread), &none);
        assert!(unread.contains(&RUE_TRUST.to_string()), "edited since read");
        assert!(!unread.contains(&ADA_TIDES.to_string()), "read");
        // A read post stays while it's sticky.
        let sticky = HashSet::from([("sub-ada".to_string(), ADA_TIDES.to_string())]);
        assert!(ids(Source::Smart(Smart::Unread), &sticky).contains(&ADA_TIDES.to_string()));
        assert_eq!(ids(Source::Smart(Smart::All), &none).len(), rows.len());
    }

    #[test]
    fn today_is_the_posts_own_date_in_local_time() {
        let (sd, now) = seed();
        let mut r = sd.reading[0].clone();
        r.created = Some(now.to_rfc3339());
        r.updated = None;
        assert!(is_today(&r, now));
        r.created = Some((now - chrono::Duration::days(3)).to_rfc3339());
        assert!(!is_today(&r, now));
        // Imported today, dated nothing: not "today".
        r.created = None;
        r.observed_at = now.to_rfc3339();
        assert!(!is_today(&r, now));
    }

    #[test]
    fn stepping_and_letters() {
        let (sd, now) = seed();
        let e = entries(&sd.folders, &sd.filed, &sd.subs, &[], &HashSet::new(), now);
        let all = Source::Smart(Smart::All);
        assert_eq!(
            step_source(&e, &all, 1),
            Some(Source::Folder(FOLDER_FRIENDS.into())),
            "headings are skipped"
        );
        assert_eq!(
            step_source(&e, &Source::Smart(Smart::Unread), -1),
            Some(Source::Smart(Smart::Unread)),
            "clamped"
        );
        assert_eq!(letter("Omar's notes", ""), "O");
        assert_eq!(letter("", "https://zed.example.com/"), "Z");
        assert_eq!(letter("  ", ""), "·");
    }

    #[test]
    fn space_finds_the_next_post_to_read() {
        let (sd, _) = seed();
        let rows = super::super::vm::order(sd.reading.clone());
        let all: Vec<usize> = (0..rows.len()).collect();
        let first = next_unread(&rows, &all, None).unwrap();
        assert!(needs_reading(&rows[first]));
        let second = next_unread(&rows, &all, Some(first)).unwrap();
        assert!(second > first && needs_reading(&rows[second]));
        assert_eq!(next_unread(&rows, &all, Some(rows.len() - 1)), None);
    }
}
