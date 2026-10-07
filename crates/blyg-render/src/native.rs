//! Markdown → a small block model for native (non-HTML) rendering: the
//! reading stream draws posts with GPUI text, no WebView per post.
//!
//! The same markdown-it engine as the HTML renderer parses the text, so the
//! structure agrees with what a blyg publishes: paragraphs, headings, block
//! quotes, lists, code, rules, and inline emphasis, code and links. What
//! can't be drawn as text becomes a placeholder block: an image, a YouTube
//! embed, a table. In a thread, a `![[id]]` line (outside code) is a
//! [`Block::Transclusion`] for the caller to fill from what it holds, and an
//! inline `[[id]]` link (outside code) is a [`Span`] with `item` set, for
//! the caller to label. TK scopes are shown as their output (what the
//! published page shows).
//!
//! This never feeds the HTML renderer, so it can't change published output.

use markdown_it::Node;
use markdown_it::parser::inline::{Text, TextSpecial};
use markdown_it::plugins::cmark::block::blockquote::Blockquote;
use markdown_it::plugins::cmark::block::code::CodeBlock;
use markdown_it::plugins::cmark::block::fence::CodeFence;
use markdown_it::plugins::cmark::block::heading::ATXHeading;
use markdown_it::plugins::cmark::block::hr::ThematicBreak;
use markdown_it::plugins::cmark::block::lheading::SetextHeader;
use markdown_it::plugins::cmark::block::list::{BulletList, OrderedList};
use markdown_it::plugins::cmark::block::paragraph::Paragraph;
use markdown_it::plugins::cmark::inline::autolink::Autolink;
use markdown_it::plugins::cmark::inline::backticks::CodeInline;
use markdown_it::plugins::cmark::inline::emphasis::{Em, Strong};
use markdown_it::plugins::cmark::inline::image::Image;
use markdown_it::plugins::cmark::inline::link::Link;
use markdown_it::plugins::cmark::inline::newline::{Hardbreak, Softbreak};
use markdown_it::plugins::extra::strikethrough::Strikethrough;
use markdown_it::plugins::extra::tables::Table;

use crate::Kind;
use crate::markdown::{Linkified, YtFacade};

/// A run of inline text with one style.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub em: bool,
    pub strong: bool,
    pub code: bool,
    pub strike: bool,
    /// The link target, when this run is (inside) a link.
    pub link: Option<String>,
    /// An `[[id]]` internal link: the target's id. `text` is a neutral
    /// label ([`LINK_LABEL`]) for the caller to replace with the target's
    /// excerpt when it holds the target.
    pub item: Option<String>,
}

/// The text of an `[[id]]` span before the caller labels it.
pub const LINK_LABEL: &str = "linked post";

/// One block of a post.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Para(Vec<Span>),
    /// Level 1–6.
    Heading(u8, Vec<Span>),
    Quote(Vec<Block>),
    List {
        /// `Some(start)` for a numbered list.
        ordered: Option<u32>,
        items: Vec<Vec<Block>>,
    },
    Code(String),
    Rule,
    /// An image, shown as a compact placeholder.
    Image {
        alt: String,
        src: String,
    },
    /// A YouTube link on its own line (the published page's facade).
    Embed {
        url: String,
    },
    /// A table, shown as a placeholder.
    Table,
    /// `![[id]]` on its own line in a thread. `excerpt` is the passage of a
    /// partial quote (the `>` run attached with no blank line, spec §16.4),
    /// as plain text with paragraphs split by `\n`.
    Transclusion {
        id: String,
        excerpt: Option<String>,
    },
}

impl Block {
    /// The block's plain text (spans joined), for line estimates and tests.
    pub fn plain(&self) -> String {
        fn spans(s: &[Span]) -> String {
            s.iter().map(|s| s.text.as_str()).collect()
        }
        match self {
            Block::Para(s) | Block::Heading(_, s) => spans(s),
            Block::Quote(b) => b.iter().map(Block::plain).collect::<Vec<_>>().join("\n"),
            Block::List { items, .. } => items
                .iter()
                .map(|i| i.iter().map(Block::plain).collect::<Vec<_>>().join(" "))
                .collect::<Vec<_>>()
                .join("\n"),
            Block::Code(c) => c.clone(),
            Block::Rule | Block::Table | Block::Embed { .. } => String::new(),
            Block::Image { alt, .. } => alt.clone(),
            Block::Transclusion { id, .. } => format!("![[{id}]]"),
        }
    }
}

/// Parse `md` into blocks. `kind` decides whether `![[id]]` lines are quotes
/// (threads) or literal text (fragments), as publishing does.
pub fn native_blocks(md: &str, kind: Kind) -> Vec<Block> {
    let (text, ids) = crate::links::tokenize(&tk_output(md));
    let out = blocks_of(&text, kind);
    if ids.is_empty() {
        out
    } else {
        out.into_iter().map(|b| untoken_block(b, &ids)).collect()
    }
}

/// Turn link tokens back into `[[id]]` spans (or, where only plain text
/// fits, the label).
fn untoken_block(b: Block, ids: &[String]) -> Block {
    let spans = |v: Vec<Span>| merge(v.into_iter().flat_map(|s| untoken_span(s, ids)).collect());
    let text = |t: String| {
        crate::links::split_tokens(&t, ids)
            .into_iter()
            .map(|p| p.unwrap_or(LINK_LABEL))
            .collect::<String>()
    };
    match b {
        Block::Para(s) => Block::Para(spans(s)),
        Block::Heading(l, s) => Block::Heading(l, spans(s)),
        Block::Quote(bs) => Block::Quote(bs.into_iter().map(|b| untoken_block(b, ids)).collect()),
        Block::List { ordered, items } => Block::List {
            ordered,
            items: items
                .into_iter()
                .map(|i| i.into_iter().map(|b| untoken_block(b, ids)).collect())
                .collect(),
        },
        Block::Image { alt, src } => Block::Image {
            alt: text(alt),
            src: text(src),
        },
        Block::Code(c) => Block::Code(text(c)),
        Block::Embed { url } => Block::Embed { url: text(url) },
        b @ (Block::Rule | Block::Table | Block::Transclusion { .. }) => b,
    }
}

fn untoken_span(s: Span, ids: &[String]) -> Vec<Span> {
    if !s.text.contains(crate::links::NATIVE_SENTINEL) {
        return vec![s];
    }
    crate::links::split_tokens(&s.text, ids)
        .into_iter()
        .map(|p| match p {
            Ok(t) => Span {
                text: t.to_string(),
                ..s.clone()
            },
            Err(id) => Span {
                text: LINK_LABEL.to_string(),
                item: Some(id.to_string()),
                ..s.clone()
            },
        })
        .collect()
}

fn blocks_of(text: &str, kind: Kind) -> Vec<Block> {
    let mut out = Vec::new();
    if kind == Kind::Thread && text.contains("![[") {
        let lines: Vec<&str> = text.split('\n').collect();
        let code = crate::markdown::code_lines(text);
        let mut prose: Vec<&str> = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            let line = lines[i];
            let directive = (!code.get(i).copied().unwrap_or(false))
                .then(|| directive_id(line))
                .flatten();
            i += 1;
            match directive {
                Some(id) => {
                    parse_into(&prose.join("\n"), &mut out);
                    prose.clear();
                    // The attached quote is part of the directive, not the
                    // author's own blockquote.
                    let mut run = Vec::new();
                    while let Some(q) = lines.get(i).filter(|l| l.trim_start().starts_with('>')) {
                        let q = q.trim_start()[1..].to_string();
                        run.push(q.strip_prefix([' ', '\t']).map(str::to_string).unwrap_or(q));
                        i += 1;
                    }
                    let excerpt = (!run.is_empty())
                        .then(|| crate::selection_from_quote(&run.join("\n")))
                        .filter(|e| !e.is_empty());
                    out.push(Block::Transclusion { id, excerpt });
                }
                None => prose.push(line),
            }
        }
        parse_into(&prose.join("\n"), &mut out);
    } else {
        parse_into(text, &mut out);
    }
    out
}

/// The id of a `![[id]]` line. Looser than the publishing grammar (any case,
/// any letters): this only decides where a quote box goes on screen.
fn directive_id(line: &str) -> Option<String> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r"^\s*!\[\[([0-9A-Za-z]{26})\]\]\s*$").expect("directive re")
    });
    re.captures(line).map(|c| c[1].to_string())
}

/// `[TK]instruction[=]output[/TK]` → `output` (an ungenerated scope shows
/// nothing), the Worker's linear scan: no nesting, and an unterminated
/// scope ends the scan.
fn tk_output(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut rest = md;
    while let Some(open) = rest.find("[TK]") {
        let after = &rest[open + 4..];
        let Some(close) = after.find("[/TK]") else {
            break;
        };
        out.push_str(&rest[..open]);
        let body = &after[..close];
        if let Some(eq) = body.find("[=]") {
            out.push_str(&body[eq + 3..]);
        }
        rest = &after[close + 5..];
    }
    out.push_str(rest);
    out
}

fn parse_into(src: &str, out: &mut Vec<Block>) {
    if src.trim().is_empty() {
        return;
    }
    let root = crate::markdown::parse_tree(src);
    blocks(&root.children, out);
}

fn blocks(nodes: &[Node], out: &mut Vec<Block>) {
    for n in nodes {
        if n.is::<Paragraph>() {
            para(n, out);
        } else if let Some(h) = n.cast::<ATXHeading>() {
            out.push(Block::Heading(h.level, inline(&n.children)));
        } else if let Some(h) = n.cast::<SetextHeader>() {
            out.push(Block::Heading(h.level, inline(&n.children)));
        } else if n.is::<Blockquote>() {
            let mut inner = Vec::new();
            blocks(&n.children, &mut inner);
            out.push(Block::Quote(inner));
        } else if n.is::<BulletList>() || n.is::<OrderedList>() {
            let ordered = n.cast::<OrderedList>().map(|o| o.start);
            let items = n
                .children
                .iter()
                .map(|li| {
                    let mut b = Vec::new();
                    blocks(&li.children, &mut b);
                    b
                })
                .collect();
            out.push(Block::List { ordered, items });
        } else if let Some(c) = n.cast::<CodeFence>() {
            out.push(Block::Code(c.content.trim_end_matches('\n').to_string()));
        } else if let Some(c) = n.cast::<CodeBlock>() {
            out.push(Block::Code(c.content.trim_end_matches('\n').to_string()));
        } else if n.is::<ThematicBreak>() {
            out.push(Block::Rule);
        } else if let Some(y) = n.cast::<YtFacade>() {
            out.push(Block::Embed {
                url: y.href.clone(),
            });
        } else if n.is::<Table>() {
            out.push(Block::Table);
        } else if !n.children.is_empty() {
            // Anything else with content (a tight list item's inline, a
            // reference definition's leftovers): keep its blocks or text.
            let before = out.len();
            blocks(&n.children, out);
            if out.len() == before {
                let spans = inline(&n.children);
                if spans.iter().any(|s| !s.text.trim().is_empty()) {
                    out.push(Block::Para(spans));
                }
            }
        } else if let Some(t) = n.cast::<Text>() {
            // Tight list items hold their inline text directly.
            if !t.content.trim().is_empty() {
                out.push(Block::Para(vec![Span {
                    text: t.content.clone(),
                    ..Default::default()
                }]));
            }
        }
    }
}

/// A paragraph: images standing alone become their own placeholder blocks,
/// so a photo between two lines of text doesn't render as a word.
fn para(n: &Node, out: &mut Vec<Block>) {
    let mut run: Vec<&Node> = Vec::new();
    let flush = |run: &mut Vec<&Node>, out: &mut Vec<Block>| {
        let mut spans = Vec::new();
        for k in run.iter() {
            walk_inline(k, &Span::default(), &mut spans);
        }
        let spans = merge(trim_spans(spans));
        if spans.iter().any(|s| !s.text.trim().is_empty()) {
            out.push(Block::Para(spans));
        }
        run.clear();
    };
    for k in &n.children {
        if let Some(img) = k.cast::<Image>() {
            flush(&mut run, out);
            out.push(Block::Image {
                alt: inline_text(&k.children),
                src: img.url.clone(),
            });
        } else {
            run.push(k);
        }
    }
    flush(&mut run, out);
}

fn inline(nodes: &[Node]) -> Vec<Span> {
    let mut spans = Vec::new();
    for n in nodes {
        walk_inline(n, &Span::default(), &mut spans);
    }
    merge(trim_spans(spans))
}

fn inline_text(nodes: &[Node]) -> String {
    inline(nodes).into_iter().map(|s| s.text).collect()
}

fn walk_inline(n: &Node, style: &Span, out: &mut Vec<Span>) {
    let push = |out: &mut Vec<Span>, text: &str| {
        out.push(Span {
            text: text.to_string(),
            ..style.clone()
        });
    };
    if let Some(t) = n.cast::<Text>() {
        push(out, &t.content);
        return;
    }
    if let Some(t) = n.cast::<TextSpecial>() {
        push(out, &t.content);
        return;
    }
    if n.is::<Softbreak>() {
        push(out, " ");
        return;
    }
    if n.is::<Hardbreak>() {
        push(out, "\n");
        return;
    }
    if n.is::<Image>() {
        // Inside a link or a heading: a word-sized placeholder.
        let alt = inline_text(&n.children);
        let label = if alt.trim().is_empty() {
            "[image]".to_string()
        } else {
            format!("[image: {}]", alt.trim())
        };
        push(out, &label);
        return;
    }
    let mut s = style.clone();
    if n.is::<Em>() {
        s.em = true;
    } else if n.is::<Strong>() {
        s.strong = true;
    } else if n.is::<Strikethrough>() {
        s.strike = true;
    } else if n.is::<CodeInline>() {
        s.code = true;
    } else if let Some(l) = n.cast::<Link>() {
        s.link = Some(l.url.clone());
    } else if let Some(l) = n.cast::<Autolink>() {
        s.link = Some(l.url.clone());
    } else if let Some(l) = n.cast::<Linkified>() {
        s.link = Some(l.url.clone());
    }
    for k in &n.children {
        walk_inline(k, &s, out);
    }
}

/// Drop leading/trailing whitespace of the whole run.
fn trim_spans(mut spans: Vec<Span>) -> Vec<Span> {
    while let Some(f) = spans.first_mut() {
        let t = f.text.trim_start().to_string();
        if t.is_empty() {
            spans.remove(0);
        } else {
            f.text = t;
            break;
        }
    }
    while let Some(l) = spans.last_mut() {
        let t = l.text.trim_end().to_string();
        if t.is_empty() {
            spans.pop();
        } else {
            l.text = t;
            break;
        }
    }
    spans
}

/// Join neighbouring runs of the same style.
fn merge(spans: Vec<Span>) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::with_capacity(spans.len());
    for s in spans {
        match out.last_mut() {
            Some(l)
                if l.em == s.em
                    && l.strong == s.strong
                    && l.code == s.code
                    && l.strike == s.strike
                    && l.link == s.link
                    && l.item.is_none()
                    && s.item.is_none() =>
            {
                l.text.push_str(&s.text)
            }
            _ => out.push(s),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(s: &str) -> Span {
        Span {
            text: s.into(),
            ..Default::default()
        }
    }

    #[test]
    fn paragraphs_emphasis_links_and_code() {
        let b = native_blocks(
            "Hello *soft* **loud** `x` [there](https://a.example/)\nnext line",
            Kind::Fragment,
        );
        assert_eq!(b.len(), 1);
        let Block::Para(s) = &b[0] else { panic!() };
        assert_eq!(s[0], plain("Hello "));
        assert!(s[1].em && s[1].text == "soft");
        assert!(s[3].strong && s[3].text == "loud");
        assert!(s[5].code && s[5].text == "x");
        let link = s.iter().find(|s| s.link.is_some()).unwrap();
        assert_eq!(link.text, "there");
        assert_eq!(link.link.as_deref(), Some("https://a.example/"));
        assert!(b[0].plain().ends_with("there next line"));
    }

    #[test]
    fn bare_urls_are_links() {
        let b = native_blocks("see https://a.example/x for more", Kind::Fragment);
        let Block::Para(s) = &b[0] else { panic!() };
        assert!(
            s.iter()
                .any(|s| s.link.as_deref() == Some("https://a.example/x"))
        );
    }

    #[test]
    fn structure() {
        let md = "# Title\n\nPara.\n\n> quoted\n> more\n\n- one\n- two\n\n3. three\n4. four\n\n```\ncode\n```\n\n---\n\n![A heron](media/h.png)\n\nhttps://www.youtube.com/watch?v=dQw4w9WgXcQ\n\n| a | b |\n|---|---|\n| 1 | 2 |";
        let b = native_blocks(md, Kind::Thread);
        assert!(matches!(&b[0], Block::Heading(1, s) if s[0].text == "Title"));
        assert_eq!(b[1].plain(), "Para.");
        assert!(matches!(&b[2], Block::Quote(q) if q[0].plain() == "quoted more"));
        assert!(
            matches!(&b[3], Block::List { ordered: None, items } if items.len() == 2 && items[1][0].plain() == "two")
        );
        assert!(matches!(
            &b[4],
            Block::List {
                ordered: Some(3),
                ..
            }
        ));
        assert_eq!(b[5], Block::Code("code".into()));
        assert_eq!(b[6], Block::Rule);
        assert_eq!(
            b[7],
            Block::Image {
                alt: "A heron".into(),
                src: "media/h.png".into()
            }
        );
        assert!(matches!(&b[8], Block::Embed { url } if url.contains("youtube")));
        assert_eq!(b[9], Block::Table);
    }

    #[test]
    fn transclusions_only_in_threads_and_never_in_code() {
        let id = "01k2ada0tides0000000000001";
        let md = format!("Before\n![[{id}]]\nAfter\n\n```\n![[{id}]]\n```");
        let t = native_blocks(&md, Kind::Thread);
        assert_eq!(t[0].plain(), "Before");
        assert_eq!(
            t[1],
            Block::Transclusion {
                id: id.into(),
                excerpt: None
            }
        );
        assert_eq!(t[2].plain(), "After");
        assert!(matches!(&t[3], Block::Code(c) if c.contains("![[")));
        let f = native_blocks(&md, Kind::Fragment);
        assert!(!f.iter().any(|b| matches!(b, Block::Transclusion { .. })));
    }

    #[test]
    fn an_attached_quote_is_the_excerpt_and_a_blank_line_detaches() {
        let id = "01k2ada0tides0000000000001";
        let md = format!("![[{id}]]\n> One *line*\n>\n> two\nMine");
        let t = native_blocks(&md, Kind::Thread);
        assert_eq!(
            t[0],
            Block::Transclusion {
                id: id.into(),
                excerpt: Some("One line\ntwo".into())
            }
        );
        assert_eq!(t[1].plain(), "Mine");
        assert_eq!(t.len(), 2);

        let md = format!("![[{id}]]\n\n> my own quote");
        let t = native_blocks(&md, Kind::Thread);
        assert!(matches!(&t[0], Block::Transclusion { excerpt: None, .. }));
        assert!(matches!(&t[1], Block::Quote(_)));

        // An empty run is consumed, and shows the whole post.
        let t = native_blocks(&format!("![[{id}]]\n> \nAfter"), Kind::Thread);
        assert!(matches!(&t[0], Block::Transclusion { excerpt: None, .. }));
        assert_eq!(t[1].plain(), "After");
    }

    #[test]
    fn tk_scopes_show_their_output() {
        let b = native_blocks("A [TK]say hi[=]friendly hello[/TK] to all", Kind::Fragment);
        assert_eq!(b[0].plain(), "A friendly hello to all");
    }

    #[test]
    fn inline_images_are_words() {
        let b = native_blocks("text ![a pic](x.png) more", Kind::Fragment);
        assert_eq!(b.len(), 3);
        assert_eq!(b[0].plain(), "text");
        assert!(matches!(&b[1], Block::Image { alt, .. } if alt == "a pic"));
        let b = native_blocks("[![logo](x.png)](https://a.example/)", Kind::Fragment);
        assert_eq!(b[0].plain(), "[image: logo]");
    }

    #[test]
    fn empty_is_empty() {
        assert!(native_blocks("", Kind::Thread).is_empty());
        assert!(native_blocks("\n\n  \n", Kind::Fragment).is_empty());
    }

    #[test]
    fn internal_links_are_item_spans() {
        let id = "01j9zq3k4m5n6p7q8r9s0t1v2w";
        let md = format!("See [[{id}]] and *[[{id}]]*, not `[[{id}]]` or ![[{id}]].");
        let b = native_blocks(&md, Kind::Fragment);
        let Block::Para(s) = &b[0] else {
            panic!("{b:?}")
        };
        let items: Vec<_> = s.iter().filter(|s| s.item.is_some()).collect();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].text, LINK_LABEL);
        assert!(items[1].em);
        assert_eq!(items[0].item.as_deref(), Some(id));
        let plain = b[0].plain();
        assert!(plain.contains(&format!("`[[{id}]]`")) || plain.contains(&format!("[[{id}]]")));
        assert!(plain.contains(&format!("![[{id}]]")));
        assert!(!plain.contains('\u{4}'));
        // Part of an autolink's URL: not a link of its own.
        let b = native_blocks(&format!("<https://x.test/[[{id}]]>"), Kind::Fragment);
        let Block::Para(s) = &b[0] else {
            panic!("{b:?}")
        };
        assert!(s.iter().all(|s| s.item.is_none()), "{s:?}");
    }
}
