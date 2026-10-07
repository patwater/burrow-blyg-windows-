//! Lineage (⌘J): a post's neighbours one step each way, from what this Mac
//! holds. What it draws on (its `stub_of`, `forked_from` and quotes) is
//! exact; what draws on it is everything seen here: reading rows, your own
//! published posts, and verified mentions of your posts. The same rules as
//! blygger-studio's lineage view (one reference per post and target, fork >
//! stub > quote, a passage quote marks it partial), but presence only: no
//! counts anywhere (spec rule 1). Pure model; `lineage.rs` draws it.

use std::collections::HashMap;

use blyg_core::profile::Relation;
use blyg_core::{Item, Kind, Mention, PostRef, ReadingItem, Response, post_key};

use super::vm;

// ------------------------------------------------------------------ kinds

/// One kind of relation on one side of a post: absent, only quotes of a
/// passage, or at least one of the whole post.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mark {
    #[default]
    None,
    Partial,
    Whole,
}

/// Which kinds of relation a post has on one side. Never how many.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Kinds {
    pub fork: Mark,
    pub stub: Mark,
    pub quote: Mark,
}

impl Kinds {
    pub fn add(&mut self, rel: Relation, partial: bool) {
        let m = match rel {
            Relation::Forks => &mut self.fork,
            Relation::Stubs => &mut self.stub,
            Relation::Quotes => &mut self.quote,
        };
        *m = match (*m, partial) {
            (Mark::Whole, _) | (_, false) => Mark::Whole,
            _ => Mark::Partial,
        };
    }

    pub fn get(&self, rel: Relation) -> Mark {
        match rel {
            Relation::Forks => self.fork,
            Relation::Stubs => self.stub,
            Relation::Quotes => self.quote,
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Kinds::default()
    }

    pub fn of(refs: &[PostRef]) -> Kinds {
        let mut k = Kinds::default();
        for r in refs {
            k.add(r.relation, r.partial);
        }
        k
    }

    /// The kinds in words, for the glyph's tooltip: "a reply to, quotes".
    pub fn words(&self, up: bool) -> Vec<&'static str> {
        let mut out = Vec::new();
        for rel in [Relation::Forks, Relation::Stubs, Relation::Quotes] {
            let m = self.get(rel);
            if m == Mark::None {
                continue;
            }
            let partial = m == Mark::Partial;
            out.push(match (up, rel, partial) {
                (true, Relation::Forks, _) => "forks a post",
                (true, Relation::Stubs, false) => "replies to a post",
                (true, Relation::Stubs, true) => "replies to a passage",
                (true, Relation::Quotes, false) => "quotes posts",
                (true, Relation::Quotes, true) => "quotes passages",
                (false, Relation::Forks, _) => "forked",
                (false, Relation::Stubs, false) => "replied to",
                (false, Relation::Stubs, true) => "replied to a passage of",
                (false, Relation::Quotes, false) => "quoted",
                (false, Relation::Quotes, true) => "quoted a passage of",
            });
        }
        out
    }
}

/// The relation a mention names (`stub` | `transclusion` | `fork`).
pub fn mention_relation(rel: Option<&str>) -> Option<Relation> {
    match rel? {
        "stub" => Some(Relation::Stubs),
        "fork" => Some(Relation::Forks),
        "transclusion" => Some(Relation::Quotes),
        _ => None,
    }
}

fn shown_mention(m: &Mention) -> bool {
    m.status == "verified" && !m.hidden
}

/// What draws on each post (by [`post_key`]), as kinds: the reading rows'
/// references, your own posts' (`own_refs`), and verified mentions of your
/// posts. One entry per post that has any.
pub fn down_index(
    rows: &[ReadingItem],
    own_refs: &[PostRef],
    mentions: &[Mention],
    own_origin: Option<&str>,
) -> HashMap<(String, String), Kinds> {
    let mut out: HashMap<(String, String), Kinds> = HashMap::new();
    for r in rows
        .iter()
        .flat_map(|r| r.references())
        .chain(own_refs.iter().cloned())
    {
        out.entry((r.origin, r.id))
            .or_default()
            .add(r.relation, r.partial);
    }
    if let Some(own) = own_origin {
        for m in mentions.iter().filter(|m| shown_mention(m)) {
            if let Some(rel) = mention_relation(m.relation.as_deref()) {
                out.entry(post_key(own, &m.target_item_id))
                    .or_default()
                    .add(rel, false);
            }
        }
    }
    out
}

// ------------------------------------------------------------------ the view's model

/// One neighbour of the centre.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub origin: String,
    pub id: String,
    pub relation: Relation,
    pub partial: bool,
    pub title: String,
    /// The author, or "you".
    pub who: String,
    /// The post as held here, when it is.
    pub held: Option<ReadingItem>,
}

/// The post in the middle.
#[derive(Debug, Clone, PartialEq)]
pub struct Centre {
    pub origin: String,
    pub id: String,
    pub title: String,
    pub who: String,
    pub kind: Option<Kind>,
    /// Held in the reading list.
    pub held: Option<ReadingItem>,
    /// One of your published posts.
    pub own: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub centre: Centre,
    /// What the centre draws on, in the order it names them.
    pub ups: Vec<Node>,
    /// What draws on it, newest first.
    pub downs: Vec<Node>,
}

/// What the model is built from.
pub struct Sources<'a> {
    pub rows: &'a [ReadingItem],
    pub own: &'a [Item],
    pub own_origin: Option<&'a str>,
    pub mentions: &'a [Mention],
}

impl Sources<'_> {
    fn held(&self, origin: &str, id: &str) -> Option<&ReadingItem> {
        let key = post_key(origin, id);
        self.rows
            .iter()
            .find(|r| post_key(&r.origin, &r.remote_id) == key)
    }

    fn own_item(&self, origin: &str, id: &str) -> Option<&Item> {
        let own = self.own_origin?;
        if post_key(own, "").0 != post_key(origin, "").0 {
            return None;
        }
        self.own.iter().find(|i| {
            i.version > 0
                && i.server_id
                    .as_ref()
                    .is_some_and(|s| s.0.eq_ignore_ascii_case(id))
        })
    }
}

fn who_of(r: &ReadingItem) -> String {
    if r.subscription_id.is_empty() && r.subscription_title == "you" {
        return "you".into();
    }
    r.author
        .as_ref()
        .and_then(|a| a.name.clone())
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| r.subscription_title.clone())
}

/// Build the view of `(origin, id)`. `responses` is `Backend::responses`
/// for it (reading rows and your own posts).
pub fn build(origin: &str, id: &str, src: &Sources, responses: Vec<Response>) -> Model {
    let held = src.held(origin, id).cloned();
    let own = src.own_item(origin, id);
    let centre = match (&held, own) {
        (Some(r), _) => Centre {
            origin: r.origin.clone(),
            id: r.remote_id.clone(),
            title: vm::post_title(r),
            who: who_of(r),
            kind: Some(r.kind),
            held: Some(r.clone()),
            own: false,
        },
        (None, Some(i)) => Centre {
            origin: origin.to_string(),
            id: id.to_string(),
            title: i.title(),
            who: "you".into(),
            kind: Some(i.kind),
            held: None,
            own: true,
        },
        (None, None) => Centre {
            origin: origin.to_string(),
            id: id.to_string(),
            title: format!("A post on {}", vm::host(origin)),
            who: vm::host(origin),
            kind: None,
            held: None,
            own: false,
        },
    };

    // What it draws on: its own references, described as well as we can.
    let refs: Vec<PostRef> = match (&held, own) {
        (Some(r), _) => r.references(),
        (None, Some(i)) => i.references(src.own_origin.unwrap_or(origin), src.rows),
        _ => vec![],
    };
    let cited = |rid: &str| {
        held.as_ref().and_then(|r| {
            r.transclusions
                .iter()
                .find(|t| t.id.eq_ignore_ascii_case(rid))
                .and_then(|t| t.cited.clone())
        })
    };
    let ups = refs
        .into_iter()
        .map(|r| {
            let h = src.held(&r.origin, &r.id).cloned();
            let mine = src.own_item(&r.origin, &r.id);
            let c = cited(&r.id);
            let (title, who) = match (&h, mine) {
                (Some(h), _) => (vm::post_title(h), who_of(h)),
                (None, Some(i)) => (i.title(), "you".into()),
                (None, None) => (
                    c.as_ref()
                        .and_then(|c| c.excerpt.clone().or(c.source.clone()))
                        .unwrap_or_else(|| format!("A post on {}", vm::host(&r.origin))),
                    c.as_ref()
                        .and_then(|c| c.author.clone())
                        .unwrap_or_else(|| vm::host(&r.origin)),
                ),
            };
            Node {
                origin: r.origin,
                id: r.id,
                relation: r.relation,
                partial: r.partial,
                title,
                who,
                held: h,
            }
        })
        .collect();

    // What draws on it: responses held here, then verified mentions of
    // your post that no held post accounts for.
    let mut downs: Vec<Node> = responses
        .into_iter()
        .map(|r| Node {
            origin: r.item.origin.clone(),
            id: r.item.remote_id.clone(),
            relation: r.relation,
            partial: r.partial,
            title: vm::post_title(&r.item),
            who: who_of(&r.item),
            held: (!r.item.subscription_id.is_empty()).then_some(r.item),
        })
        .collect();
    if centre.own {
        for m in src
            .mentions
            .iter()
            .filter(|m| shown_mention(m) && m.target_item_id.eq_ignore_ascii_case(&centre.id))
        {
            let (Some(rel), Some(sid)) = (mention_relation(m.relation.as_deref()), &m.source_id)
            else {
                continue;
            };
            let sorigin = m.source_origin.clone().unwrap_or_else(|| m.source.clone());
            let key = post_key(&sorigin, sid);
            if downs.iter().any(|d| post_key(&d.origin, &d.id) == key) {
                continue;
            }
            let h = src.held(&sorigin, sid).cloned();
            downs.push(Node {
                title: h
                    .as_ref()
                    .map(vm::post_title)
                    .unwrap_or_else(|| format!("A post on {}", vm::host(&sorigin))),
                who: m
                    .source_author
                    .as_ref()
                    .and_then(|a| a.name.clone())
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| vm::host(&sorigin)),
                origin: sorigin,
                id: sid.clone(),
                relation: rel,
                partial: false,
                held: h,
            });
        }
    }
    Model { centre, ups, downs }
}

/// How a neighbour relates, from its side of the edge.
pub fn relation_words(rel: Relation, partial: bool, up: bool) -> &'static str {
    match (up, rel, partial) {
        (true, Relation::Forks, _) => "forked from",
        (true, Relation::Stubs, false) => "replies to",
        (true, Relation::Stubs, true) => "replies to a passage of",
        (true, Relation::Quotes, false) => "quotes",
        (true, Relation::Quotes, true) => "quotes a passage of",
        (false, Relation::Forks, _) => "forked this",
        (false, Relation::Stubs, false) => "replied",
        (false, Relation::Stubs, true) => "replied to a passage",
        (false, Relation::Quotes, false) => "quoted this",
        (false, Relation::Quotes, true) => "quoted a passage",
    }
}

// ------------------------------------------------------------------ selection

/// What the keyboard is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sel {
    Centre,
    Up(usize),
    Down(usize),
}

impl Model {
    /// The neighbour `sel` is on (`None` on the centre).
    pub fn node(&self, sel: Sel) -> Option<&Node> {
        match sel {
            Sel::Centre => None,
            Sel::Up(i) => self.ups.get(i),
            Sel::Down(i) => self.downs.get(i),
        }
    }

    /// Where an arrow key moves `sel`.
    pub fn step(&self, sel: Sel, key: &str) -> Sel {
        let mid = |n: usize| n / 2;
        match (sel, key) {
            (Sel::Centre, "up") if !self.ups.is_empty() => Sel::Up(mid(self.ups.len())),
            (Sel::Centre, "down") if !self.downs.is_empty() => Sel::Down(mid(self.downs.len())),
            (Sel::Up(_), "down") | (Sel::Down(_), "up") => Sel::Centre,
            (Sel::Up(i), "left") => Sel::Up(i.saturating_sub(1)),
            (Sel::Up(i), "right") => Sel::Up((i + 1).min(self.ups.len().saturating_sub(1))),
            (Sel::Down(i), "left") => Sel::Down(i.saturating_sub(1)),
            (Sel::Down(i), "right") => Sel::Down((i + 1).min(self.downs.len().saturating_sub(1))),
            _ => sel,
        }
    }
}

// ------------------------------------------------------------------ the ring

/// The reader's actions, in the ring's fixed places (the same compass as
/// blygger-studio's): bottom puts their words in yours, the sides make
/// your own thing, the top only looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Fork,
    Reply,
    Quote,
    Link,
    Versions,
    Open,
}

/// The four facts every action answers, in order.
pub const FACTS: [&str; 4] = [
    "a response?",
    "the author is told?",
    "their words in yours",
    "shows under their post?",
];

impl Act {
    pub const ALL: [Act; 6] = [
        Act::Fork,
        Act::Reply,
        Act::Quote,
        Act::Link,
        Act::Versions,
        Act::Open,
    ];

    /// Degrees clockwise from east (screen coordinates).
    pub fn angle(self) -> f32 {
        match self {
            Act::Fork => 0.,
            Act::Reply => 60.,
            Act::Quote => 120.,
            Act::Link => 180.,
            Act::Versions => 240.,
            Act::Open => 300.,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Act::Fork => "f",
            Act::Reply => "r",
            Act::Quote => "q",
            Act::Link => "l",
            Act::Versions => "v",
            Act::Open => "o",
        }
    }

    pub fn of_key(k: &str) -> Option<Act> {
        Act::ALL.into_iter().find(|a| a.key() == k)
    }

    pub fn label(self) -> &'static str {
        match self {
            Act::Fork => "Fork",
            Act::Reply => "Reply",
            Act::Quote => "Quote",
            Act::Link => "Link post",
            Act::Versions => "Versions",
            Act::Open => "Open ↗",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Act::Fork => "Fork this pin",
            Act::Reply => "Reply",
            Act::Quote => "Quote",
            Act::Link => "Link post",
            Act::Versions => "Versions",
            Act::Open => "Open on web",
        }
    }

    /// The relation its result has to the centre (colours the wedge).
    pub fn relation(self) -> Option<Relation> {
        match self {
            Act::Fork => Some(Relation::Forks),
            Act::Reply => Some(Relation::Stubs),
            Act::Quote => Some(Relation::Quotes),
            _ => None,
        }
    }

    /// The node it would add below the centre: (title, subtitle).
    pub fn ghost(self, passage: bool) -> Option<(&'static str, &'static str)> {
        match self {
            Act::Fork => Some(("your fork", "draft · yours to edit")),
            Act::Reply if passage => Some(("your reply, to a passage", "new thread")),
            Act::Reply => Some(("your reply", "new thread")),
            Act::Quote => Some(("your thread", "quoting this")),
            Act::Link => Some(("your fragment", "…links here…")),
            _ => None,
        }
    }

    pub fn one(self) -> &'static str {
        match self {
            Act::Fork => "Start your own copy of this post.",
            Act::Reply => "Respond to this post.",
            Act::Quote => "Quote it in a thread you're writing (⌘K).",
            Act::Link => "Write something of your own that links to this.",
            Act::Versions => "See how this post has changed.",
            Act::Open => "Read it on the author's own site (⌘O).",
        }
    }

    pub fn long(self) -> &'static str {
        match self {
            Act::Fork => {
                "Opens a draft that starts as the author's last pinned version, flattened to \
                 text you can rewrite freely. It's yours, and it records that it was forked \
                 from theirs."
            }
            Act::Reply => {
                "Opens a thread that starts with the post quoted as a frozen snapshot (or just \
                 the passage you selected), with your reply beneath it. The author's blyg is \
                 told, and can list it under their post."
            }
            Act::Quote => {
                "Adds ![[id]] to the thread you're writing (or a new one), which shows the post \
                 inside yours. It isn't a reply, but the author's blyg is told it was quoted."
            }
            Act::Link => {
                "Opens a fragment that starts with [[id]], which publishes as an ordinary link. \
                 Nothing is copied and nothing is sent: the author isn't told."
            }
            Act::Versions => {
                "The post's versions, with the author's change notes. Between pinned versions \
                 you can see the change word by word. Nothing is created."
            }
            Act::Open => {
                "Opens the post's public page in your browser. Whatever you're writing stays \
                 where it is."
            }
        }
    }

    /// The four facts: (answer, yes?).
    pub fn facts(self, passage: bool) -> [(&'static str, bool); 4] {
        match self {
            Act::Fork => [
                ("no, a line of its own", false),
                ("yes, as a fork", true),
                ("all of them, editable", true),
                ("listed as a fork", true),
            ],
            Act::Reply => [
                ("yes", true),
                ("yes, as a reply (stub)", true),
                (
                    if passage {
                        "just the passage you selected"
                    } else {
                        "the whole post, frozen"
                    },
                    true,
                ),
                ("yes, as a response", true),
            ],
            Act::Quote => [
                ("no, part of your thread", false),
                ("yes, as a quote", true),
                ("the whole post, frozen", true),
                ("yes, as a quote", true),
            ],
            Act::Link => [
                ("no", false),
                ("no, links are silent", false),
                ("none", false),
                ("no", false),
            ],
            Act::Versions | Act::Open => [
                ("n/a, reading only", false),
                ("no", false),
                ("none", false),
                ("n/a", false),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blyg_core::{RemoteRef, ServerId, Status, StubOf, TransclusionRef};

    const ADA: &str = "https://ada.example/";
    const BO: &str = "https://bo.example/";
    const ME: &str = "https://me.example/";

    fn row(origin: &str, id: &str, md: &str) -> ReadingItem {
        serde_json::from_value(serde_json::json!({
            "subscription_id": "S", "remote_id": id, "subscription_title": vm::host(origin),
            "origin": origin, "kind": "thread", "state": "current", "version": 1,
            "created": null, "updated": null, "observed_at": "2030-01-01T00:00:00Z",
            "content_md": md, "content_html": "", "author": null, "page": null,
            "thumb": null, "hoppers": [],
        }))
        .unwrap()
    }

    fn mine(id: &str, md: &str) -> Item {
        Item {
            local_id: blyg_core::LocalId(format!("L{id}")),
            server_id: Some(ServerId(id.into())),
            kind: Kind::Thread,
            status: Status::Public,
            version: 1,
            dirty: false,
            content_md: md.into(),
            created: "2030-01-01T00:00:00Z".into(),
            updated: "2030-01-01T00:00:00Z".into(),
            permalink: None,
            stub_of: None,
            forked_from: None,
            show_responses: true,
            responses_mode: None,
            pending_sync: false,
            conflict: false,
            highlight: false,
            highlight_mode: None,
        }
    }

    #[test]
    fn kinds_say_presence_never_how_many() {
        let mut k = Kinds::default();
        k.add(Relation::Stubs, true);
        assert_eq!(k.stub, Mark::Partial);
        k.add(Relation::Stubs, false);
        k.add(Relation::Stubs, true);
        assert_eq!(k.stub, Mark::Whole);
        assert_eq!(k.fork, Mark::None);
        assert_eq!(k.words(false), ["replied to"]);
        for w in Kinds::default().words(true) {
            assert!(!w.chars().any(|c| c.is_ascii_digit()));
        }
    }

    #[test]
    fn the_down_index_takes_rows_your_posts_and_mentions() {
        let mut reply = row(BO, "R", "Re");
        reply.stub_of = Some(StubOf {
            origin: Some(ADA.into()),
            id: Some("T".into()),
            version: Some(1),
            url: None,
        });
        let (o, i) = post_key(ADA, "T");
        let own = vec![PostRef {
            origin: o,
            id: i,
            relation: Relation::Forks,
            version: Some(1),
            partial: false,
        }];
        let m = Mention {
            id: "m".into(),
            target_item_id: "MINE".into(),
            status: "verified".into(),
            relation: Some("transclusion".into()),
            source: "https://bo.example/t/x".into(),
            source_origin: Some(BO.into()),
            source_id: Some("X".into()),
            source_kind: None,
            source_version: None,
            source_author: None,
            first_seen: "t".into(),
            verified_at: None,
            hidden: false,
        };
        let idx = down_index(&[reply], &own, &[m], Some(ME));
        let t = idx[&post_key(ADA, "T")];
        assert_eq!(
            (t.fork, t.stub, t.quote),
            (Mark::Whole, Mark::Whole, Mark::None)
        );
        assert_eq!(idx[&post_key(ME, "mine")].quote, Mark::Whole);
    }

    #[test]
    fn the_model_has_one_step_each_way() {
        const T: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        let mut centre = row(ADA, T, "Tides\nbody");
        centre.forked_from = Some(RemoteRef {
            origin: BO.into(),
            id: "K".into(),
            version: 2,
        });
        centre.transclusions = vec![TransclusionRef {
            id: "Z".into(),
            version: Some(1),
            origin: Some("https://far.example/".into()),
            cited: Some(blyg_core::Cited {
                source: Some("Far".into()),
                author: Some("Fay".into()),
                excerpt: Some("A far post".into()),
                ..Default::default()
            }),
            selector: None,
        }];
        let kit = row(BO, "K", "Kit's original");
        let own = mine("Q1", &format!("Mine\n![[{T}]]"));
        let rows = [centre.clone(), kit];
        let src = Sources {
            rows: &rows,
            own: std::slice::from_ref(&own),
            own_origin: Some(ME),
            mentions: &[],
        };
        let responses = blyg_core::model::own_responses(std::slice::from_ref(&own), ME, ADA, T);
        let m = build(ADA, &T.to_lowercase(), &src, responses);
        assert_eq!(m.centre.title, "Tides");
        let ups: Vec<(&str, Relation, &str)> = m
            .ups
            .iter()
            .map(|n| (n.who.as_str(), n.relation, n.title.as_str()))
            .collect();
        assert_eq!(
            ups,
            [
                ("bo.example", Relation::Forks, "Kit's original"),
                ("Fay", Relation::Quotes, "A far post"),
            ]
        );
        assert_eq!(m.downs.len(), 1);
        assert_eq!(
            (m.downs[0].who.as_str(), m.downs[0].relation),
            ("you", Relation::Quotes)
        );
        // Arrows: up to the middle of the row above, along it, back down.
        assert_eq!(m.step(Sel::Centre, "up"), Sel::Up(1));
        assert_eq!(m.step(Sel::Up(1), "left"), Sel::Up(0));
        assert_eq!(m.step(Sel::Up(0), "left"), Sel::Up(0));
        assert_eq!(m.step(Sel::Up(0), "down"), Sel::Centre);
        assert_eq!(m.step(Sel::Centre, "down"), Sel::Down(0));
    }

    #[test]
    fn the_ring_has_six_keys_in_fixed_places() {
        let keys: Vec<&str> = Act::ALL.iter().map(|a| a.key()).collect();
        assert_eq!(keys, ["f", "r", "q", "l", "v", "o"]);
        for (i, a) in Act::ALL.iter().enumerate() {
            assert_eq!(a.angle(), i as f32 * 60.);
            assert_eq!(Act::of_key(a.key()), Some(*a));
        }
        assert_eq!(Act::Reply.facts(true)[2].0, "just the passage you selected");
    }
}
