//! `![[id]]` → a quote, from the local store only: your own items
//! (`Backend::items`) first, then imported reading items
//! (`Backend::reading`), in the Worker's `resolveTarget` order. Nothing is
//! ever fetched (SPEC § Protocol philosophy 3).
//!
//! A snapshot of both lists is taken on the UI thread (local, instant) and
//! moved to the render thread, so rendering never touches the backend.

use std::cell::RefCell;
use std::collections::HashMap;

use blyg_core::{Backend, Item, Kind, ReadingItem, Status, SubscriptionKind};
use blyg_render::{
    Found, ItemKind, RenderOpts, Resolution, Resolver, UnresolvedReason, render_preview,
};

/// How deep a quoted thread's own quotes are baked (a quote of a quote of a
/// quote is still shown; deeper ones become the unresolved marker).
const MAX_DEPTH: usize = 3;

pub struct StoreResolver {
    own: HashMap<String, Item>,
    remote: HashMap<String, Vec<(ReadingItem, bool)>>,
    mount: String,
    /// Ids being baked right now (cycle and depth guard).
    stack: RefCell<Vec<String>>,
}

fn item_kind(k: Kind) -> ItemKind {
    match k {
        Kind::Fragment => ItemKind::Fragment,
        Kind::Thread => ItemKind::Thread,
    }
}

pub fn render_kind(k: Kind) -> blyg_render::Kind {
    match k {
        Kind::Fragment => blyg_render::Kind::Fragment,
        Kind::Thread => blyg_render::Kind::Thread,
    }
}

impl StoreResolver {
    /// `mount` prefixes provenance links to your own items (`{mount}/f/{id}/`).
    pub fn new(
        items: Vec<Item>,
        reading: Vec<ReadingItem>,
        rss_subs: &[String],
        mount: &str,
    ) -> Self {
        let own = items
            .into_iter()
            .filter_map(|i| i.server_id.clone().map(|s| (s.0, i)))
            .collect();
        let mut remote: HashMap<String, Vec<(ReadingItem, bool)>> = HashMap::new();
        for r in reading {
            let rss = rss_subs.contains(&r.subscription_id);
            remote
                .entry(r.remote_id.clone())
                .or_default()
                .push((r, rss));
        }
        StoreResolver {
            own,
            remote,
            mount: mount.to_string(),
            stack: RefCell::new(Vec::new()),
        }
    }

    /// Everything the backend holds locally.
    pub fn snapshot(backend: &dyn Backend, mount: &str) -> Self {
        let rss: Vec<String> = backend
            .subscriptions()
            .into_iter()
            .filter(|s| s.kind == SubscriptionKind::Rss)
            .map(|s| s.id)
            .collect();
        Self::new(backend.items(), backend.reading(), &rss, mount)
    }

    /// Resolves nothing (a document without `![[`: no snapshot needed).
    pub fn empty(mount: &str) -> Self {
        Self::new(Vec::new(), Vec::new(), &[], mount)
    }

    /// Your own published item, baked as its snapshot would be.
    fn own(&self, id: &str, item: &Item) -> Resolution {
        match item.status {
            // A scratch note is never on the server: as unquotable as a draft.
            Status::Draft | Status::Scratch => {
                return Resolution::Unavailable(UnresolvedReason::Draft);
            }
            Status::Withdrawn => return Resolution::Unavailable(UnresolvedReason::Withdrawn),
            Status::Public => {}
        }
        if self.stack.borrow().iter().any(|s| s == id) {
            return Resolution::Unavailable(UnresolvedReason::Circular);
        }
        if self.stack.borrow().len() >= MAX_DEPTH {
            return Resolution::Unavailable(UnresolvedReason::Other(
                "quoted too deeply to preview".into(),
            ));
        }
        self.stack.borrow_mut().push(id.to_string());
        let opts = RenderOpts {
            data_line: false,
            provenance: true,
            mount: self.mount.clone(),
            self_id: Some(id.to_string()),
        };
        // The local copy is the working copy; for a post with unpublished
        // edits that is newer than the published snapshot publish would bake.
        let html = render_preview(&item.content_md, render_kind(item.kind), self, &opts).html;
        self.stack.borrow_mut().pop();
        Resolution::Found(Found {
            origin: None,
            id: id.to_string(),
            version: item.version,
            kind: item_kind(item.kind),
            content_html: html,
            author: None,
            page: None,
        })
    }

    fn imported(&self, id: &str, rows: &[(ReadingItem, bool)]) -> Resolution {
        let mut origins: Vec<&str> = rows.iter().map(|(r, _)| r.origin.as_str()).collect();
        origins.sort_unstable();
        origins.dedup();
        if origins.len() > 1 {
            return Resolution::Ambiguous;
        }
        let (r, rss) = &rows[0];
        if *rss {
            return Resolution::RssNotQuotable;
        }
        let version = if r.state == "tombstone" {
            match r.pinned_version_retained {
                Some(v) => v,
                None => return Resolution::Unavailable(UnresolvedReason::SourceWithdrawn),
            }
        } else {
            r.version
        };
        Resolution::Found(Found {
            origin: Some(r.origin.clone()),
            id: id.to_string(),
            version,
            kind: item_kind(r.kind),
            // Stored verbatim (spec §14: sanitize at render); the page CSP
            // is the backstop.
            content_html: super::sanitize::sanitize(&r.content_html),
            author: r
                .author
                .as_ref()
                .and_then(|a| a.name.clone())
                .filter(|n| !n.trim().is_empty()),
            page: r.page.clone(),
        })
    }
}

impl Resolver for StoreResolver {
    fn resolve(&self, id: &str) -> Resolution {
        if let Some(item) = self.own.get(id) {
            return self.own(id, item);
        }
        match self.remote.get(id) {
            Some(rows) if !rows.is_empty() => self.imported(id, rows),
            _ => Resolution::NotFound,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blyg_core::{Author, LocalId, ServerId};

    const OWN: &str = "0123456789abcdefghjkmnpqrs";
    const DRAFT: &str = "0123456789abcdefghjkmnpqrt";
    const THEIRS: &str = "zyxwvtsrqpnmkjhgfedcba9876";

    fn item(id: &str, status: Status, md: &str) -> Item {
        Item {
            local_id: LocalId(format!("local-{id}")),
            server_id: Some(ServerId(id.into())),
            kind: Kind::Fragment,
            status,
            version: 2,
            dirty: false,
            content_md: md.into(),
            created: String::new(),
            updated: String::new(),
            permalink: None,
            stub_of: None,
            forked_from: None,
            show_responses: true,
            pending_sync: false,
            conflict: false,
        }
    }

    fn reading(id: &str, origin: &str, sub: &str) -> ReadingItem {
        ReadingItem {
            subscription_id: sub.into(),
            remote_id: id.into(),
            subscription_title: "Field Notes".into(),
            origin: origin.into(),
            kind: Kind::Fragment,
            state: "current".into(),
            version: 4,
            created: None,
            updated: None,
            observed_at: String::new(),
            content_md: "Tides keep time.".into(),
            content_html: "<p>Tides keep time.</p>\n".into(),
            author: Some(Author {
                name: Some("Field Notes".into()),
                url: None,
            }),
            page: None,
            thumb: None,
            hoppers: vec![],
            pinned_version_retained: None,
            read_version: None,
            stub_of: None,
            forked_from: None,
            transclusions: vec![],
        }
    }

    fn quote(r: &StoreResolver, id: &str) -> blyg_render::Rendered {
        render_preview(
            &format!("Intro\n\n![[{id}]]\n"),
            blyg_render::Kind::Thread,
            r,
            &RenderOpts::default(),
        )
    }

    #[test]
    fn own_items_first_then_imported() {
        let r = StoreResolver::new(
            vec![
                item(OWN, Status::Public, "Soil *matters*."),
                item(DRAFT, Status::Draft, "not yet"),
            ],
            vec![
                reading(THEIRS, "https://notes.example.org/", "s1"),
                // The same id imported too: own wins.
                reading(OWN, "https://notes.example.org/", "s1"),
            ],
            &[],
            "https://blyg.example.com",
        );
        let Resolution::Found(f) = r.resolve(OWN) else {
            panic!("own")
        };
        assert_eq!(f.origin, None);
        assert_eq!(f.version, 2);
        assert_eq!(f.content_html, "<p>Soil <em>matters</em>.</p>\n");

        let Resolution::Found(f) = r.resolve(THEIRS) else {
            panic!("imported")
        };
        assert_eq!(f.origin.as_deref(), Some("https://notes.example.org/"));
        assert_eq!(f.author.as_deref(), Some("Field Notes"));
        assert_eq!(f.version, 4);

        assert_eq!(
            r.resolve(DRAFT),
            Resolution::Unavailable(UnresolvedReason::Draft)
        );
        assert_eq!(
            r.resolve("00000000000000000000000000"),
            Resolution::NotFound
        );

        let out = quote(&r, THEIRS);
        assert_eq!(out.stats.quotes, 1);
        assert!(out.html.contains("blyg-transclusion"));
        assert!(out.html.contains("Tides keep time."));
        let out = quote(&r, DRAFT);
        assert_eq!(out.stats.unresolved.len(), 1);
        assert_eq!(out.stats.unresolved[0].reason, UnresolvedReason::Draft);
    }

    #[test]
    fn ambiguous_rss_and_withdrawn_sources() {
        let mut tomb = reading(THEIRS, "https://notes.example.org/", "s1");
        tomb.state = "tombstone".into();
        let r = StoreResolver::new(vec![], vec![tomb.clone()], &[], "/blyg");
        assert_eq!(
            r.resolve(THEIRS),
            Resolution::Unavailable(UnresolvedReason::SourceWithdrawn)
        );
        tomb.pinned_version_retained = Some(3);
        let r = StoreResolver::new(vec![], vec![tomb], &[], "/blyg");
        assert!(matches!(r.resolve(THEIRS), Resolution::Found(f) if f.version == 3));

        let r = StoreResolver::new(
            vec![],
            vec![reading(THEIRS, "https://feed.example.org/", "rss1")],
            &["rss1".to_string()],
            "/blyg",
        );
        assert_eq!(r.resolve(THEIRS), Resolution::RssNotQuotable);

        let r = StoreResolver::new(
            vec![],
            vec![
                reading(THEIRS, "https://notes.example.org/", "s1"),
                reading(THEIRS, "https://copy.example.net/", "s2"),
            ],
            &[],
            "/blyg",
        );
        assert_eq!(r.resolve(THEIRS), Resolution::Ambiguous);
    }

    #[test]
    fn quoted_threads_nest_without_cycles() {
        let mut a = item(OWN, Status::Public, &format!("A quotes B\n\n![[{DRAFT}]]"));
        a.kind = Kind::Thread;
        let mut b = item(DRAFT, Status::Public, &format!("B quotes A\n\n![[{OWN}]]"));
        b.kind = Kind::Thread;
        let r = StoreResolver::new(vec![a, b], vec![], &[], "/blyg");
        let out = quote(&r, OWN);
        assert_eq!(out.stats.quotes, 1);
        // B is baked inside A; B's quote of A is circular.
        assert!(out.html.contains("B quotes A"));
        assert!(out.html.contains("unresolved"));
    }

    #[test]
    fn empty_resolves_nothing() {
        let r = StoreResolver::empty("/blyg");
        assert_eq!(r.resolve(OWN), Resolution::NotFound);
    }
}
