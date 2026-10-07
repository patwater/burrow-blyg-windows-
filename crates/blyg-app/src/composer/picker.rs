//! The `[[` (link) / `![[` (quote) picker (studio 0.29), the parts both
//! of its forms share: when typing opens it, the query typed after the
//! brackets, what a pick inserts, the remembered filters, and the filter
//! chips and result rows.
//!
//! Where the query is typed is the blyg's `picker_typing` setting: in the
//! editor after the brackets (`auto` on a Mac, and `editor`), where the
//! picker is a popup under the caret run by `Assist`; or in the picker's
//! own box (`panel`), the sheet in `reading/quote_picker.rs`, which ⌘K
//! always opens. The search is local: `Backend::pick_search`.

use std::rc::Rc;

use blyg_core::{Kind, PickQuery, PickSort, PickSource, Pickable, Subscription, SubscriptionKind};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::text::{in_code_fence, in_inline_code};
use crate::theme::Palette;

/// What the picker inserts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickKind {
    /// `[[id]]`, inline: a link that declares nothing on the wire.
    Link,
    /// `![[id]]` on its own line: a transclusion (threads only).
    Quote,
}

impl PickKind {
    pub fn open(self) -> &'static str {
        match self {
            PickKind::Link => "[[",
            PickKind::Quote => "![[",
        }
    }

    pub fn heading(self) -> &'static str {
        match self {
            PickKind::Link => "Link to a post",
            PickKind::Quote => "Quote in this thread",
        }
    }

    pub fn enter_hint(self) -> &'static str {
        match self {
            PickKind::Link => "inserts [[…]]",
            PickKind::Quote => "inserts ![[…]] on its own line",
        }
    }
}

/// The filters, remembered while the app runs (upstream keeps them per
/// device): one for every editor and both forms of the picker.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filters {
    pub source: PickSource,
    pub sort: PickSort,
    /// Only this subscription's posts (with `Imported`).
    pub sub: Option<String>,
}

impl Global for Filters {}

impl Filters {
    pub fn get(cx: &App) -> Filters {
        cx.try_global::<Filters>().cloned().unwrap_or_default()
    }

    pub fn set(self, cx: &mut App) {
        cx.set_global(self);
    }

    pub fn query(&self, text: &str, exclude: Option<String>) -> PickQuery {
        PickQuery {
            text: text.to_string(),
            source: self.source,
            sub: self
                .sub
                .clone()
                .filter(|_| self.source == PickSource::Imported),
            sort: self.sort,
            exclude,
            limit: 0,
        }
    }
}

/// If `old -> new` typed one `[` at `cursor` that makes a `[[` (not `![[`,
/// not `[[[`), outside code, the byte offset of that `[[` in `new`. A
/// paste never opens it.
pub fn link_trigger(old: &str, new: &str, cursor: usize) -> Option<usize> {
    if new.len() != old.len() + 1 || cursor < 2 || cursor > new.len() {
        return None;
    }
    if !new.is_char_boundary(cursor) || !new[..cursor].ends_with("[[") {
        return None;
    }
    if new[..cursor - 1] != old[..cursor - 1] || new[cursor..] != old[cursor - 1..] {
        return None;
    }
    let at = cursor - 2;
    if new[..at].ends_with('!') || new[..at].ends_with('[') {
        return None;
    }
    let line_start = new[..at].rfind('\n').map_or(0, |i| i + 1);
    if in_code_fence(&new[..line_start]) || in_inline_code(&new[line_start..at]) {
        return None;
    }
    Some(at)
}

/// The query typed after the brackets that start at `at`, while the caret
/// is still there: on the same line, with no `]` yet.
pub fn query(text: &str, at: usize, kind: PickKind, cursor: usize) -> Option<&str> {
    let open = kind.open();
    if text.get(at..at + open.len()) != Some(open) {
        return None;
    }
    let start = at + open.len();
    if cursor < start || cursor > text.len() {
        return None;
    }
    let q = text.get(start..cursor)?;
    (!q.contains(['\n', ']', '[']) && q.chars().count() <= 80).then_some(q)
}

/// Replace the typed brackets and query (`at..cursor`, plus a `]]` the
/// editor may already hold after it) with the pick. A link goes inline; a
/// quote takes its line. Returns the new text and the caret.
pub fn insert(text: &str, at: usize, cursor: usize, kind: PickKind, id: &str) -> (String, usize) {
    let at = at.min(text.len());
    let mut end = cursor.min(text.len()).max(at);
    if text[end..].starts_with("]]") {
        end += 2;
    }
    let directive = format!("{}{id}]]", kind.open());
    let mut out = String::with_capacity(text.len() + directive.len() + 1);
    out.push_str(&text[..at]);
    out.push_str(&directive);
    let mut caret = out.len();
    if kind == PickKind::Quote && !text[end..].starts_with('\n') {
        out.push('\n');
        caret += 1;
    } else if kind == PickKind::Quote {
        caret += 1;
    }
    out.push_str(&text[end..]);
    let caret = caret.min(out.len());
    (out, caret)
}

/// The blyg subscriptions, for the "from" chip: (id, title).
pub fn blyg_subs(subs: &[Subscription]) -> Vec<(String, String)> {
    subs.iter()
        .filter(|s| s.kind == SubscriptionKind::Blyg)
        .map(|s| {
            let t = s.title.trim();
            let t = if t.is_empty() {
                crate::vm::url_host(&s.origin).unwrap_or_else(|| s.origin.clone())
            } else {
                t.to_string()
            };
            (s.id.clone(), t)
        })
        .collect()
}

/// The row's second line: `yours · thread · v2 · 3d` / `Rue · fragment · v1`.
pub fn meta(p: &Pickable, now: chrono::DateTime<chrono::Utc>) -> String {
    let who = match p.source {
        PickSource::Mine => "yours".to_string(),
        _ => p.source_title.clone().unwrap_or_default(),
    };
    let kind = match p.kind {
        Kind::Fragment => "fragment",
        Kind::Thread => "thread",
    };
    let when = crate::vm::relative_time(&p.at, now);
    format!("{who} · {kind} · v{} · {when}", p.version)
}

/// Sets the filters (and searches again).
pub type OnFilters = Rc<dyn Fn(Filters, &mut Window, &mut App)>;
/// Picks row `i`.
pub type OnPick = Rc<dyn Fn(usize, &mut Window, &mut App)>;

fn chip(p: Palette, id: SharedString, label: String, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(7.))
        .py(px(1.))
        .rounded(px(5.))
        .border_1()
        .border_color(if on { p.accent } else { p.edge() })
        .when(on, |d| d.text_color(p.accent))
        .when(!on, |d| d.text_color(p.muted))
        .cursor_pointer()
        .hover(|s| s.bg(p.hover()))
        .child(label)
}

/// Source (all · mine · imported), the subscription under imported (a
/// click steps through them), and sort.
pub fn filter_row(p: Palette, f: &Filters, subs: &[(String, String)], set: OnFilters) -> Div {
    let mut row = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(5.))
        .text_size(px(11.));
    for (src, label) in [
        (PickSource::All, "all"),
        (PickSource::Mine, "mine"),
        (PickSource::Imported, "imported"),
    ] {
        let set = set.clone();
        let next = Filters {
            source: src,
            ..f.clone()
        };
        row = row.child(
            chip(
                p,
                format!("pick-src-{}", src.as_str()).into(),
                label.into(),
                f.source == src,
            )
            .on_click(move |_, window, cx| set(next.clone(), window, cx)),
        );
    }
    if f.source == PickSource::Imported && !subs.is_empty() {
        let at = f
            .sub
            .as_ref()
            .and_then(|s| subs.iter().position(|(id, _)| id == s));
        let label = match at {
            Some(i) => format!("from {} ▸", subs[i].1),
            None => "from every source ▸".into(),
        };
        let next_sub = match at {
            None => Some(subs[0].0.clone()),
            Some(i) => subs.get(i + 1).map(|(id, _)| id.clone()),
        };
        let next = Filters {
            sub: next_sub,
            ..f.clone()
        };
        let set = set.clone();
        row = row.child(
            chip(p, "pick-sub".into(), label, at.is_some())
                .on_click(move |_, window, cx| set(next.clone(), window, cx)),
        );
    }
    let (label, other) = match f.sort {
        PickSort::Newest => ("newest first", PickSort::Oldest),
        PickSort::Oldest => ("oldest first", PickSort::Newest),
    };
    let next = Filters {
        sort: other,
        ..f.clone()
    };
    row.child(div().flex_1()).child(
        chip(p, "pick-sort".into(), format!("{label} ⇅"), false)
            .on_click(move |_, window, cx| set(next.clone(), window, cx)),
    )
}

/// The result rows: title, excerpt, and where it's from.
pub fn rows(p: Palette, rows: &[Pickable], sel: usize, pick: OnPick) -> Vec<AnyElement> {
    let now = chrono::Utc::now();
    rows.iter()
        .enumerate()
        .map(|(i, r)| {
            let pick = pick.clone();
            // The excerpt after the title (it usually starts with it).
            let rest = r
                .excerpt
                .strip_prefix(r.title.as_str())
                .unwrap_or(&r.excerpt)
                .trim();
            let excerpt = (!rest.is_empty()).then(|| rest.to_string());
            div()
                .id(("pick-row", i))
                .px(px(8.))
                .py(px(4.))
                .rounded(px(5.))
                .cursor_pointer()
                .when(i == sel, |d| d.bg(p.pick()))
                .when(i != sel, |d| d.hover(|s| s.bg(p.hover())))
                .on_click(move |_, window, cx| pick(i, window, cx))
                .child(div().truncate().child(r.title.clone()))
                .children(excerpt.map(|e| {
                    div()
                        .text_size(px(11.5))
                        .text_color(p.muted)
                        .truncate()
                        .child(e)
                }))
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(p.muted)
                        .truncate()
                        .child(meta(r, now)),
                )
                .into_any_element()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{Filters, PickKind, insert, link_trigger, query};
    use blyg_core::{PickSort, PickSource};

    fn typed(before: &str, after: &str) -> Option<usize> {
        let old = format!("{}{after}", &before[..before.len() - 1]);
        let new = format!("{before}{after}");
        link_trigger(&old, &new, before.len())
    }

    #[test]
    fn link_trigger_fires_on_a_typed_bracket_pair() {
        assert_eq!(typed("[[", ""), Some(0));
        assert_eq!(typed("see [[", " there"), Some(4));
        assert_eq!(typed("mid[[", ""), Some(3));
        assert_eq!(typed("![[", ""), None, "that's a quote");
        assert_eq!(typed("[[[", ""), None);
        assert_eq!(typed("[", ""), None);
        assert_eq!(typed("`code [[", "`"), None, "inline code");
        assert_eq!(typed("```\n[[", "\n```"), None, "fenced code");
        assert_eq!(link_trigger("", "[[x]]", 2), None, "a paste");
    }

    #[test]
    fn the_query_is_what_follows_the_brackets() {
        let t = "see [[garden be";
        assert_eq!(query(t, 4, PickKind::Link, t.len()), Some("garden be"));
        assert_eq!(query(t, 4, PickKind::Link, 6), Some(""));
        assert_eq!(query(t, 4, PickKind::Link, 5), None, "caret left");
        assert_eq!(query("see [[a]] b", 4, PickKind::Link, 11), None);
        assert_eq!(query("[[a\nb", 0, PickKind::Link, 5), None);
        assert_eq!(query("![[ti", 0, PickKind::Quote, 5), Some("ti"));
        assert_eq!(query("![[ti", 0, PickKind::Link, 5), None);
    }

    #[test]
    fn a_pick_replaces_the_typed_text() {
        let (t, c) = insert("see [[gar more", 4, 9, PickKind::Link, "01ABC");
        assert_eq!(t, "see [[01ABC]] more");
        assert_eq!(&t[..c], "see [[01ABC]]");
        let (t, _) = insert("see [[gar]] more", 4, 9, PickKind::Link, "01ABC");
        assert_eq!(t, "see [[01ABC]] more", "a closing ]] isn't doubled");
        let (t, c) = insert("One.\n![[tid", 5, 11, PickKind::Quote, "01Q");
        assert_eq!(t, "One.\n![[01Q]]\n");
        assert_eq!(c, t.len());
        let (t, c) = insert("One.\n![[t\nTwo.", 5, 9, PickKind::Quote, "01Q");
        assert_eq!(t, "One.\n![[01Q]]\nTwo.");
        assert_eq!(&t[..c], "One.\n![[01Q]]\n");
    }

    #[test]
    fn filters_make_the_query() {
        let f = Filters {
            source: PickSource::Mine,
            sort: PickSort::Oldest,
            sub: Some("s1".into()),
        };
        let q = f.query("beds", Some("X".into()));
        assert_eq!(q.sub, None, "a subscription only with imported");
        assert_eq!(q.sort, PickSort::Oldest);
        assert_eq!(q.exclude.as_deref(), Some("X"));
        let f = Filters {
            source: PickSource::Imported,
            ..f
        };
        assert_eq!(f.query("", None).sub.as_deref(), Some("s1"));
    }
}
