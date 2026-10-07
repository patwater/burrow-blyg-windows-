//! Responses to a post (issue #7): who quoted, stubbed or forked it, as a
//! list and never a count (spec rule 1). Two sources:
//!
//! - your own posts: the verified mentions your blyg already collects
//!   (`GET /api/mentions`);
//! - anyone's post: posts in your reading list whose `transclusions`,
//!   `stub_of` or `forked_from` point at it ("seen in your network",
//!   `Backend::responses`, an index lookup in the store).
//!
//! The list sits at the bottom of the reading pane (and under your own post
//! in ⌘Y); the stream shows only a small "↩" marker on posts that have any.

use blyg_core::profile::Relation;
use blyg_core::{Mention, Response};
use gpui_kit::*;

use super::vm;
use crate::app::MainView;
use crate::theme::Rule; // --- themes --- dividers

/// One line of the list: who · what · when, and where a click goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseRow {
    pub who: String,
    /// "quoted this" / "stubbed this" / "forked this" (and "… a passage of
    /// this" when the quote is partial).
    pub what: &'static str,
    pub when: String,
    /// The responding post: its origin, id and version.
    pub origin: String,
    pub id: Option<String>,
    pub version: Option<u32>,
    /// Its page, when there's no id to open it by.
    pub url: Option<String>,
    at: i64,
}

fn what_of(r: Relation, partial: bool) -> &'static str {
    match (r, partial) {
        (Relation::Quotes, false) => "quoted this",
        (Relation::Quotes, true) => "quoted a passage of this",
        (Relation::Stubs, false) => "stubbed this",
        (Relation::Stubs, true) => "stubbed a passage of this",
        (Relation::Forks, _) => "forked this",
    }
}

/// How strong a row's relation is, for keeping one row per post:
/// fork > stub > quote > a bare mention.
fn strength(what: &str) -> u8 {
    match what.split(' ').next() {
        Some("forked") => 3,
        Some("stubbed") => 2,
        Some("quoted") => 1,
        _ => 0,
    }
}

fn mention_what(rel: Option<&str>) -> &'static str {
    match rel {
        Some("stub") => "stubbed this",
        Some("fork") => "forked this",
        Some("transclusion") => "quoted this",
        _ => "mentioned this",
    }
}

fn ts(s: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|t| t.timestamp_millis())
        .unwrap_or(0)
}

/// The list for the post `target_id`: verified, unhidden mentions of it
/// (your own posts) and the network's responses, one row per responding
/// post (its strongest relation, wherever it was seen), newest first.
pub fn response_rows(
    network: &[Response],
    mentions: &[Mention],
    target_id: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<ResponseRow> {
    let mut rows: Vec<ResponseRow> = Vec::new();
    for m in mentions.iter().filter(|m| {
        m.status == "verified" && !m.hidden && m.target_item_id.eq_ignore_ascii_case(target_id)
    }) {
        let origin = m.source_origin.clone().unwrap_or_else(|| m.source.clone());
        let when = m
            .verified_at
            .clone()
            .unwrap_or_else(|| m.first_seen.clone());
        rows.push(ResponseRow {
            who: m
                .source_author
                .as_ref()
                .and_then(|a| a.name.clone())
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| vm::host(&origin)),
            what: mention_what(m.relation.as_deref()),
            when: crate::vm::relative_time(&when, now),
            origin,
            id: m.source_id.clone(),
            version: m.source_version,
            url: Some(m.source.clone()),
            at: ts(&when),
        });
    }
    for r in network {
        let it = &r.item;
        let at = it.post_time().at;
        rows.push(ResponseRow {
            who: it
                .author
                .as_ref()
                .and_then(|a| a.name.clone())
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| it.subscription_title.clone()),
            what: what_of(r.relation, r.partial),
            when: crate::vm::relative_time(&at, now),
            origin: it.origin.clone(),
            id: Some(it.remote_id.clone()),
            version: Some(it.version),
            url: None,
            at: ts(&at),
        });
    }
    rows.sort_by_key(|r| std::cmp::Reverse(r.at));
    let mut seen: std::collections::HashMap<(String, String), usize> = Default::default();
    let mut out: Vec<ResponseRow> = Vec::new();
    for r in rows {
        let key = match &r.id {
            Some(id) => blyg_core::post_key(&r.origin, id),
            None => (r.url.clone().unwrap_or_default(), String::new()),
        };
        match seen.get(&key) {
            Some(&i) if strength(r.what) > strength(out[i].what) => out[i].what = r.what,
            Some(_) => {}
            None => {
                seen.insert(key, out.len());
                out.push(r);
            }
        }
    }
    out
}

impl MainView {
    /// The blyg you're connected to is `origin`.
    pub(crate) fn is_own_origin(&self, origin: &str) -> bool {
        self.backend
            .base_url()
            .is_some_and(|b| blyg_core::profile::same_origin(&b, origin))
    }

    /// This post has responses in your network (the glyph's right side).
    #[cfg(test)]
    pub(crate) fn has_responses(&self, origin: &str, id: &str) -> bool {
        self.reading
            .down
            .contains_key(&blyg_core::post_key(origin, id))
    }

    /// The responses list for `(origin, id)`; `None` when there are none.
    pub(crate) fn render_responses(
        &self,
        origin: &str,
        id: &str,
        network: &[Response],
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let mentions: &[Mention] = if self.is_own_origin(origin) {
            self.reading
                .mentions
                .ready()
                .map(Vec::as_slice)
                .unwrap_or(&[])
        } else {
            &[]
        };
        let rows = response_rows(network, mentions, id, self.now);
        if rows.is_empty() {
            return None;
        }
        let p = self.palette.on_page();
        Some(
            div()
                .id("responses")
                .debug_selector(|| "responses".into())
                .flex_none()
                .max_h(px(180.))
                .overflow_y_scroll()
                .px(px(32.))
                .pt(px(10.))
                .pb(px(6.))
                .rule_t(&p)
                .font_family(self.chrome())
                .text_size(px(12.))
                .child(
                    div()
                        .mb(px(4.))
                        .text_size(px(11.))
                        .text_color(p.muted)
                        .child("↩ Responses"),
                )
                .children(rows.into_iter().enumerate().map(|(i, r)| {
                    let (o, rid, v, url) = (r.origin.clone(), r.id.clone(), r.version, r.url);
                    div()
                        .id(("response", i))
                        .flex()
                        .gap(px(6.))
                        .py(px(2.))
                        .cursor_pointer()
                        .hover(|s| s.text_color(p.accent))
                        .child(
                            div()
                                .text_color(p.ink)
                                .font_weight(FontWeight::MEDIUM)
                                .child(r.who),
                        )
                        .child(
                            div()
                                .text_color(p.muted)
                                .child(format!("· {} · {}", r.what, r.when)),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| match (&rid, &url) {
                            (Some(id), _) => {
                                this.open_original(o.clone(), id.clone(), v, window, cx)
                            }
                            (None, Some(u)) => cx.open_url(u),
                            _ => {}
                        }))
                }))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{Mention, Relation, Response, response_rows};

    fn mention(
        id: &str,
        target: &str,
        rel: &str,
        status: &str,
        hidden: bool,
        ago_h: i64,
    ) -> Mention {
        let t = (chrono::Utc::now() - chrono::Duration::hours(ago_h)).to_rfc3339();
        Mention {
            id: id.into(),
            target_item_id: target.into(),
            status: status.into(),
            relation: Some(rel.into()),
            source: format!("https://rue.blyg.example.com/t/{id}"),
            source_origin: Some("https://rue.blyg.example.com/".into()),
            source_id: Some(id.into()),
            source_kind: Some("thread".into()),
            source_version: Some(1),
            source_author: Some(blyg_core::Author {
                name: Some("Rue".into()),
                url: None,
            }),
            first_seen: t.clone(),
            verified_at: Some(t),
            hidden,
        }
    }

    #[test]
    fn a_list_of_who_what_when_newest_first_never_a_count() {
        let now = chrono::Utc::now();
        let seed = crate::fake::reading_seed::seed(now);
        let lin = seed
            .reading
            .iter()
            .find(|r| r.remote_id == crate::fake::reading_seed::LIN_FORK)
            .unwrap()
            .clone();
        let network = vec![Response {
            item: lin,
            relation: Relation::Forks,
            version: Some(1),
            partial: false,
        }];
        let mentions = vec![
            mention("m1", "01OWN", "stub", "verified", false, 1),
            mention("m2", "01OWN", "transclusion", "pending", false, 1),
            mention("m3", "01OWN", "fork", "verified", true, 1),
            mention("m4", "01OTHER", "fork", "verified", false, 1),
            mention("m1", "01OWN", "stub", "verified", false, 1),
        ];
        let rows = response_rows(&network, &mentions, "01own", now);
        let got: Vec<(&str, &str)> = rows.iter().map(|r| (r.who.as_str(), r.what)).collect();
        assert_eq!(got, [("Rue", "stubbed this"), ("Lin", "forked this")]);
        for r in &rows {
            let words = format!("{} {}", r.who, r.what);
            assert!(!words.chars().any(|c| c.is_ascii_digit()), "{words}");
        }
        assert!(response_rows(&[], &[], "x", now).is_empty());
    }

    #[test]
    fn one_row_per_post_with_its_strongest_relation() {
        let now = chrono::Utc::now();
        let seed = crate::fake::reading_seed::seed(now);
        let mut lin = seed
            .reading
            .iter()
            .find(|r| r.remote_id == crate::fake::reading_seed::LIN_FORK)
            .unwrap()
            .clone();
        // The same post seen as a verified quote and, in the network, a stub
        // of a passage ("quote selection"): one row, the stub.
        let mut m = mention("m1", "01OWN", "transclusion", "verified", false, 1);
        m.source_origin = Some(lin.origin.clone());
        m.source_id = Some(lin.remote_id.clone());
        lin.author = None;
        lin.subscription_title = "Lin".into();
        let network = vec![Response {
            item: lin,
            relation: Relation::Stubs,
            version: Some(1),
            partial: true,
        }];
        let rows = response_rows(&network, &[m], "01own", now);
        let got: Vec<&str> = rows.iter().map(|r| r.what).collect();
        assert_eq!(got, ["stubbed a passage of this"]);
    }
}
