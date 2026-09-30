//! Markdown → HTML with the reference Worker's markdown-it semantics:
//! `new MarkdownIt({ html: false, linkify: true, typographer: false })` plus
//! its `embedsPlugin` (YouTube facade, `referrerpolicy` on off-origin images).
//!
//! Built on `markdown-it` (the Rust port of markdown-it.js), with these pieces
//! replaced to match the JS library exactly:
//! - link normalisation (`normalizeLink` / `normalizeLinkText`: mdurl parse,
//!   punycode hosts, mdurl encode/decode), which the port simplifies;
//! - linkify: both of markdown-it 14's rules (the inline `scheme://` rule and
//!   the core fuzzy rule) over a linkify-it port, where the port uses a
//!   different detector (the `linkify` crate);
//! - image `alt` text (`renderInlineAsText`, with escapes and entities kept);
//! - emphasis delimiter classification (`emph.rs`: Unicode symbols count as
//!   punctuation, as in markdown-it 14 / CommonMark 0.31);
//! - line breaks around code blocks, list items and empty blockquotes, where
//!   the port's renderer differs from markdown-it's `renderToken`.
//!
//! The result is byte-identical to the Worker on the parity corpus, all 652
//! CommonMark spec examples and the linkify test vectors (see docs/RENDER.md).

use std::sync::OnceLock;

use markdown_it::parser::inline::{InlineRule, InlineState, Text, TextSpecial};
use markdown_it::parser::linkfmt::LinkFormatter;
use markdown_it::plugins::cmark::block::blockquote::Blockquote;
use markdown_it::plugins::cmark::block::code::CodeBlock;
use markdown_it::plugins::cmark::block::fence::CodeFence;
use markdown_it::plugins::cmark::block::heading::ATXHeading;
use markdown_it::plugins::cmark::block::hr::ThematicBreak;
use markdown_it::plugins::cmark::block::lheading::SetextHeader;
use markdown_it::plugins::cmark::block::list::{BulletList, ListItem, OrderedList};
use markdown_it::plugins::cmark::block::paragraph::Paragraph;
use markdown_it::plugins::cmark::block::reference::Definition;
use markdown_it::plugins::cmark::inline::autolink::Autolink;
use markdown_it::plugins::cmark::inline::backticks::CodeInline;
use markdown_it::plugins::cmark::inline::image::Image;
use markdown_it::plugins::cmark::inline::link::Link;
use markdown_it::plugins::cmark::inline::newline::{Hardbreak, Softbreak};
use markdown_it::plugins::extra::tables::Table;
use markdown_it::{MarkdownIt, Node, NodeValue, Renderer};
use regex::Regex;

use crate::embeds::{youtube_facade_html, youtube_id};
use crate::emph;
use crate::linkify;
use crate::util::js_trim;

/// What one Markdown render produced, besides the HTML.
#[derive(Debug, Default, Clone)]
pub(crate) struct MdStats {
    pub images: usize,
    pub videos: usize,
    /// Source line (as mapped by the caller) of each top-level block, in order.
    pub block_lines: Vec<usize>,
}

fn engine() -> &'static MarkdownIt {
    static MD: OnceLock<MarkdownIt> = OnceLock::new();
    MD.get_or_init(|| {
        use markdown_it::plugins::cmark::inline::emphasis::{Em, Strong};
        use markdown_it::plugins::cmark::{block, inline};
        use markdown_it::plugins::extra::strikethrough::Strikethrough;
        let mut md = MarkdownIt::new();
        // cmark::add, with emphasis (and strikethrough) from our emph.rs.
        inline::newline::add(&mut md);
        inline::escape::add(&mut md);
        inline::backticks::add(&mut md);
        emph::add_with::<'~', 2, true>(&mut md, || Node::new(Strikethrough { marker: '~' }));
        emph::add_with::<'*', 1, true>(&mut md, || Node::new(Em { marker: '*' }));
        emph::add_with::<'_', 1, false>(&mut md, || Node::new(Em { marker: '_' }));
        emph::add_with::<'*', 2, true>(&mut md, || Node::new(Strong { marker: '*' }));
        emph::add_with::<'_', 2, false>(&mut md, || Node::new(Strong { marker: '_' }));
        inline::link::add(&mut md);
        inline::image::add(&mut md);
        inline::autolink::add(&mut md);
        inline::entity::add(&mut md);
        block::code::add(&mut md);
        block::fence::add(&mut md);
        block::blockquote::add(&mut md);
        block::hr::add(&mut md);
        block::list::add(&mut md);
        block::reference::add(&mut md);
        block::heading::add(&mut md);
        block::lheading::add(&mut md);
        block::paragraph::add(&mut md);
        markdown_it::plugins::extra::tables::add(&mut md);
        md.link_formatter = Box::new(JsLinkFormatter);
        md.inline.add_rule::<LinkifyInline>();
        md
    })
}

/// Render Markdown exactly as the Worker's `renderMarkdown` does.
///
/// With `line_of` set, every top-level block gets a `data-line` attribute:
/// `line_of(n)` maps line `n` of `src` (0-based) to the line to report.
pub(crate) fn render(src: &str, line_of: Option<&dyn Fn(usize) -> usize>) -> (String, MdStats) {
    let mut root = engine().parse(src);
    let mut stats = MdStats::default();
    linkify_core(&mut root);
    youtube_facades(&mut root);
    rewrite_images(&mut root, &mut stats);
    rewrite_code(&mut root, src);
    rewrite_containers(&mut root);
    root.walk(|n, _| {
        if n.is::<YtFacade>() {
            stats.videos += 1;
        }
    });
    if let Some(line_of) = line_of {
        let starts = line_starts(src);
        for node in root.children.iter_mut() {
            if !is_block(node) {
                continue;
            }
            let Some(map) = node.srcmap else { continue };
            let (start, _) = map.get_byte_offsets();
            let line = line_of(line_index(&starts, start));
            node.attrs.push(("data-line", line.to_string()));
            stats.block_lines.push(line);
        }
    }
    (root.render(), stats)
}

/// The engine's block rules alone: block structure never depends on inline
/// parsing, so this finds the same code blocks as [`engine`] without paying
/// for emphasis and linkify.
fn block_engine() -> &'static MarkdownIt {
    static MD: OnceLock<MarkdownIt> = OnceLock::new();
    MD.get_or_init(|| {
        use markdown_it::plugins::cmark::block;
        let mut md = MarkdownIt::new();
        block::code::add(&mut md);
        block::fence::add(&mut md);
        block::blockquote::add(&mut md);
        block::hr::add(&mut md);
        block::list::add(&mut md);
        block::reference::add(&mut md);
        block::heading::add(&mut md);
        block::lheading::add(&mut md);
        block::paragraph::add(&mut md);
        markdown_it::plugins::extra::tables::add(&mut md);
        md
    })
}

/// The Worker's `codeLines`: for each line of `src` (split on `\n`), whether
/// Markdown renders it as code (fenced or indented, at any nesting depth).
pub(crate) fn code_lines(src: &str) -> Vec<bool> {
    let starts = line_starts(src);
    let mut flags = vec![false; starts.len()];
    block_engine().parse(src).walk(|n, _| {
        if !(n.is::<CodeFence>() || n.is::<CodeBlock>()) {
            return;
        }
        if let Some(map) = n.srcmap {
            let (start, end) = map.get_byte_offsets();
            let (a, b) = (line_index(&starts, start), line_index(&starts, end));
            flags[a..=b].iter_mut().for_each(|f| *f = true);
        }
    });
    flags
}

fn is_block(node: &Node) -> bool {
    node.is::<Paragraph>()
        || node.is::<ATXHeading>()
        || node.is::<SetextHeader>()
        || node.is::<BulletList>()
        || node.is::<OrderedList>()
        || node.is::<Blockquote>()
        || node.is::<Quote>()
        || node.is::<Code>()
        || node.is::<ThematicBreak>()
        || node.is::<Table>()
        || node.is::<YtFacade>()
}

pub(crate) fn line_starts(s: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(s.match_indices('\n').map(|(i, _)| i + 1))
        .collect()
}

pub(crate) fn line_index(starts: &[usize], offset: usize) -> usize {
    match starts.binary_search(&offset) {
        Ok(i) => i,
        Err(i) => i.saturating_sub(1),
    }
}

// ---------------------------------------------------------------------------
// Link normalisation (markdown-it lib/index.mjs)

#[derive(Debug)]
struct JsLinkFormatter;

fn recode_host(url: &str, f: fn(&str) -> Option<String>) -> String {
    let mut parsed = mdurl::parse_url(url);
    if let Some(host) = parsed.hostname.as_ref() {
        let recode = match parsed.protocol.as_deref() {
            None => true,
            Some(p) => matches!(p, "http:" | "https:" | "mailto:"),
        };
        if recode && let Some(h) = f(host) {
            parsed.hostname = Some(h);
        }
    }
    parsed.to_string()
}

impl LinkFormatter for JsLinkFormatter {
    fn validate_link(&self, url: &str) -> Option<()> {
        static BAD: OnceLock<Regex> = OnceLock::new();
        static GOOD: OnceLock<Regex> = OnceLock::new();
        let bad = BAD.get_or_init(|| Regex::new(r"^(vbscript|javascript|file|data):").expect("re"));
        let good =
            GOOD.get_or_init(|| Regex::new(r"^data:image/(gif|png|jpeg|webp);").expect("re"));
        let s = js_trim(url).to_lowercase();
        if bad.is_match(&s) && !good.is_match(&s) {
            None
        } else {
            Some(())
        }
    }

    fn normalize_link(&self, url: &str) -> String {
        let formatted = recode_host(url, crate::punycode::to_ascii);
        mdurl::urlencode::encode(&formatted, mdurl::urlencode::ENCODE_DEFAULT_CHARS, true)
            .into_owned()
    }

    fn normalize_link_text(&self, url: &str) -> String {
        let formatted = recode_host(url, crate::punycode::to_unicode);
        mdurl::urlencode::decode(&formatted, mdurl::urlencode::DECODE_DEFAULT_CHARS.add(b'%'))
            .into_owned()
    }
}

// ---------------------------------------------------------------------------
// Linkify (markdown-it lib/rules_inline/linkify.mjs + rules_core/linkify.mjs)

/// A linkified URL (`link_open` with `markup: 'linkify'`).
#[derive(Debug)]
pub struct Linkified {
    pub url: String,
}

impl NodeValue for Linkified {
    fn render(&self, node: &Node, fmt: &mut dyn Renderer) {
        let mut attrs = node.attrs.clone();
        attrs.push(("href", self.url.clone()));
        fmt.open("a", &attrs);
        fmt.contents(&node.children);
        fmt.close("a");
    }
}

fn scheme_before(pending: &str) -> Option<&str> {
    // /(?:^|[^a-z0-9.+-])([a-z][a-z0-9.+-]*)$/i
    let bytes = pending.as_bytes();
    let mut start = bytes.len();
    while start > 0 {
        let c = bytes[start - 1];
        if c.is_ascii_alphanumeric() || matches!(c, b'.' | b'+' | b'-') {
            start -= 1;
        } else {
            break;
        }
    }
    // The run must start with a letter; if it does not, try later start points.
    let run = &pending[start..];
    let first_alpha = run.bytes().position(|c| c.is_ascii_alphabetic())?;
    // A later start is only valid if preceded by [^a-z0-9.+-]: inside the run
    // every byte is in that set, so only the run's own start qualifies.
    if first_alpha != 0 {
        return None;
    }
    Some(run)
}

struct LinkifyInline;

impl LinkifyInline {
    /// Returns (proto_len, url) when a `scheme://` link starts at `state.pos - proto_len`.
    fn scan(state: &InlineState) -> Option<(usize, String)> {
        if state.link_level > 0 {
            return None;
        }
        let pos = state.pos;
        let src = &state.src[..state.pos_max];
        if !src.as_bytes()[pos..].starts_with(b"://") {
            return None;
        }
        let proto = scheme_before(state.trailing_text_get())?;
        let proto_len = proto.len();
        if proto_len > pos || state.src.get(pos - proto_len..pos) != Some(proto) {
            return None;
        }
        let m = linkify::match_at_start(&state.src[pos - proto_len..])?;
        let mut url = m.url;
        if url.len() <= proto_len {
            return None;
        }
        let trimmed = url.trim_end_matches('*').len();
        url.truncate(trimmed);
        // The match must not run past this inline's range.
        if pos - proto_len + url.len() > state.pos_max {
            return None;
        }
        Some((proto_len, url))
    }
}

impl InlineRule for LinkifyInline {
    const MARKER: char = ':';

    fn check(state: &mut InlineState) -> Option<usize> {
        let (proto_len, url) = Self::scan(state)?;
        let full = state.md.link_formatter.normalize_link(&url);
        state.md.link_formatter.validate_link(&full)?;
        Some(url.len() - proto_len)
    }

    fn run(state: &mut InlineState) -> Option<(Node, usize)> {
        let (proto_len, url) = Self::scan(state)?;
        let full = state.md.link_formatter.normalize_link(&url);
        state.md.link_formatter.validate_link(&full)?;
        let content = state.md.link_formatter.normalize_link_text(&url);
        let url_start = state.pos - proto_len;
        let mut text = Node::new(Text { content });
        text.srcmap = state.get_map(url_start, url_start + url.len());
        let mut node = Node::new(Linkified { url: full });
        node.children.push(text);
        state.trailing_text_pop(proto_len);
        state.pos -= proto_len;
        Some((node, url.len()))
    }
}

fn is_linkish(node: &Node) -> bool {
    node.is::<Link>() || node.is::<Autolink>() || node.is::<Linkified>()
}

/// The core linkify rule: fuzzy links, emails and anything the inline rule
/// did not take, in every text node outside links, images and code.
fn linkify_core(node: &mut Node) {
    if is_linkish(node) || node.is::<Image>() || node.is::<CodeInline>() {
        return;
    }
    let formatter = &engine().link_formatter;
    let mut i = 0;
    while i < node.children.len() {
        let replacement = {
            let child = &node.children[i];
            match child.cast::<Text>() {
                Some(t) if linkify::pretest(&t.content) && linkify::test(&t.content) => {
                    let prev_special = i > 0 && node.children[i - 1].is::<TextSpecial>();
                    Some(split_links(
                        &t.content,
                        prev_special,
                        formatter.as_ref(),
                        child,
                    ))
                }
                _ => None,
            }
        };
        match replacement {
            Some(nodes) => {
                let n = nodes.len();
                node.children.splice(i..=i, nodes);
                i += n.max(1);
            }
            None => {
                linkify_core(&mut node.children[i]);
                i += 1;
            }
        }
    }
}

fn split_links(
    text: &str,
    prev_special: bool,
    fmt: &dyn LinkFormatter,
    original: &Node,
) -> Vec<Node> {
    let mut links = linkify::match_links(text);
    if prev_special && links.first().is_some_and(|l| l.index == 0) {
        links.remove(0);
    }
    let mut out = Vec::new();
    let mut last = 0;
    let push_text = |out: &mut Vec<Node>, s: &str| {
        let mut n = Node::new(Text {
            content: s.to_string(),
        });
        n.srcmap = original.srcmap;
        out.push(n);
    };
    for l in links {
        let full = fmt.normalize_link(&l.url);
        if fmt.validate_link(&full).is_none() {
            continue;
        }
        let url_text = if l.schema.is_empty() {
            let t = fmt.normalize_link_text(&format!("http://{}", l.text));
            t.strip_prefix("http://").map(str::to_string).unwrap_or(t)
        } else if l.schema == "mailto:" && !l.text.to_ascii_lowercase().starts_with("mailto:") {
            let t = fmt.normalize_link_text(&format!("mailto:{}", l.text));
            t.strip_prefix("mailto:").map(str::to_string).unwrap_or(t)
        } else {
            fmt.normalize_link_text(&l.text)
        };
        if l.index > last {
            push_text(&mut out, &text[last..l.index]);
        }
        let mut link = Node::new(Linkified { url: full });
        link.srcmap = original.srcmap;
        let mut t = Node::new(Text { content: url_text });
        t.srcmap = original.srcmap;
        link.children.push(t);
        out.push(link);
        last = l.last_index;
    }
    if last < text.len() {
        push_text(&mut out, &text[last..]);
    }
    out
}

// ---------------------------------------------------------------------------
// Embeds (the Worker's embeds.ts `embedsPlugin`)

/// The YouTube click-to-load facade, replacing a paragraph.
#[derive(Debug)]
pub struct YtFacade {
    pub id: String,
    pub href: String,
}

impl NodeValue for YtFacade {
    fn render(&self, node: &Node, fmt: &mut dyn Renderer) {
        fmt.cr();
        let html = youtube_facade_html(&self.id, &self.href);
        match node.attrs.iter().find(|(k, _)| *k == "data-line") {
            Some((_, line)) => {
                // Same markup, with the line attribute on the <figure>.
                let (head, rest) = html.split_at(html.find('>').expect("figure tag"));
                fmt.text_raw(head);
                fmt.text_raw(&format!(" data-line=\"{line}\""));
                fmt.text_raw(rest);
            }
            None => fmt.text_raw(&html),
        }
        fmt.cr();
    }
}

/// Link text and href when `node` is a link whose only child is its text
/// (markdown-it: `link_open`, one `text`, `link_close`).
fn bare_link(node: &Node) -> Option<(&str, String)> {
    if node.children.len() != 1 {
        return None;
    }
    let child = &node.children[0];
    if let Some(l) = node.cast::<Link>() {
        return child
            .cast::<Text>()
            .map(|t| (t.content.as_str(), l.url.clone()));
    }
    let url = if let Some(l) = node.cast::<Autolink>() {
        l.url.clone()
    } else {
        node.cast::<Linkified>()?.url.clone()
    };
    if let Some(t) = child.cast::<Text>() {
        return Some((t.content.as_str(), url));
    }
    child
        .cast::<TextSpecial>()
        .map(|t| (t.content.as_str(), url))
}

fn youtube_facades(node: &mut Node) {
    for child in node.children.iter_mut() {
        if child.is::<Paragraph>() {
            let kids: Vec<&Node> = child
                .children
                .iter()
                .filter(|k| {
                    !k.cast::<Text>()
                        .is_some_and(|t| js_trim(&t.content).is_empty())
                })
                .collect();
            if kids.len() == 1
                && let Some((text, href)) = bare_link(kids[0])
                && js_trim(text) == js_trim(&href)
                && let Some(id) = youtube_id(&href)
            {
                let srcmap = child.srcmap;
                *child = Node::new(YtFacade { id, href });
                child.srcmap = srcmap;
            }
        } else {
            youtube_facades(child);
        }
    }
}

// ---------------------------------------------------------------------------
// Images

/// markdown-it's image token, rendered with its default rule plus the
/// embeds plugin's `referrerpolicy="no-referrer"` for off-origin sources.
#[derive(Debug)]
pub struct BlygImage {
    pub url: String,
    pub title: Option<String>,
}

fn is_remote(src: &str) -> bool {
    // /^(?:https?:)?\/\//i
    let lower = src.get(..8).unwrap_or(src).to_ascii_lowercase();
    let rest = if lower.starts_with("https:") {
        &src[6..]
    } else if lower.starts_with("http:") {
        &src[5..]
    } else {
        src
    };
    rest.starts_with("//")
}

/// markdown-it `renderInlineAsText`: `text` tokens, breaks as `\n`, nested
/// images; everything else is skipped, including inline code. Escapes and
/// entities (`text_special`) count as text: the Worker's
/// `blyg_image_alt_text` rule joins them inside images, so the alt matches
/// what the same text renders as outside an image.
fn inline_as_text(nodes: &[Node], out: &mut String) {
    for n in nodes {
        if let Some(t) = n.cast::<Text>() {
            out.push_str(&t.content);
        } else if let Some(t) = n.cast::<TextSpecial>() {
            out.push_str(&t.content);
        } else if n.is::<Softbreak>() || n.is::<Hardbreak>() {
            out.push('\n');
        } else if n.is::<CodeInline>() {
            // skipped
        } else {
            inline_as_text(&n.children, out);
        }
    }
}

impl NodeValue for BlygImage {
    fn render(&self, node: &Node, fmt: &mut dyn Renderer) {
        let mut alt = String::new();
        inline_as_text(&node.children, &mut alt);
        let mut attrs = node.attrs.clone();
        attrs.push(("src", self.url.clone()));
        attrs.push(("alt", alt));
        if let Some(title) = &self.title {
            attrs.push(("title", title.clone()));
        }
        if is_remote(&self.url) {
            attrs.push(("referrerpolicy", "no-referrer".to_string()));
        }
        fmt.self_close("img", &attrs);
    }
}

fn rewrite_images(node: &mut Node, stats: &mut MdStats) {
    for child in node.children.iter_mut() {
        rewrite_images(child, stats);
        if let Some(img) = child.cast::<Image>() {
            let value = BlygImage {
                url: img.url.clone(),
                title: img.title.clone(),
            };
            child.replace(value);
            stats.images += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Code blocks

/// A fenced or indented code block, rendered as markdown-it's `fence` and
/// `code_block` rules do: no line break before `<pre>` (markdown-it.rs adds
/// one, which shows up after a tight list item's text), attributes on `<pre>`.
#[derive(Debug)]
pub struct Code {
    pub lang: Option<String>,
    pub content: String,
}

impl NodeValue for Code {
    fn render(&self, node: &Node, fmt: &mut dyn Renderer) {
        fmt.open("pre", &node.attrs);
        match &self.lang {
            Some(lang) => fmt.open("code", &[("class", format!("language-{lang}"))]),
            None => fmt.open("code", &[]),
        }
        fmt.text(&self.content);
        fmt.close("code");
        fmt.close("pre");
        fmt.cr();
    }
}

/// A fence left open that runs to the end of a document with no final
/// newline: markdown-it's `getLines` slices past the end there, so its last
/// line gets no `\n` (markdown-it.rs always adds one). The fence has no end
/// marker when every line it spans but the opening one is content.
fn unclosed_at_eof(node: &Node, content: &str, src: &str) -> bool {
    if src.ends_with('\n') || !content.ends_with('\n') {
        return false;
    }
    let Some(map) = node.srcmap else { return false };
    let (start, end) = map.get_byte_offsets();
    end == src.len() && content.matches('\n').count() == src[start..end].matches('\n').count()
}

fn rewrite_code(node: &mut Node, src: &str) {
    for child in node.children.iter_mut() {
        if let Some(f) = child.cast::<CodeFence>() {
            // info.split(/(\s+)/g)[0] after unescapeAll + trim
            let info = markdown_it::common::utils::unescape_all(&f.info);
            let lang = js_trim(&info)
                .split(crate::util::is_js_ws)
                .next()
                .unwrap_or("")
                .to_string();
            let mut content = f.content.clone();
            if unclosed_at_eof(child, &content, src) {
                content.pop();
            }
            let value = Code {
                lang: (!lang.is_empty()).then_some(lang),
                content,
            };
            child.replace(value);
        } else if let Some(c) = child.cast::<CodeBlock>() {
            let value = Code {
                lang: None,
                content: c.content.clone(),
            };
            child.replace(value);
        } else {
            rewrite_code(child, src);
        }
    }
}

// ---------------------------------------------------------------------------
// Containers: line breaks exactly where markdown-it's renderToken puts them.

/// A list item. markdown-it breaks the line after `<li>` when the first
/// child is a block (`<li>\n<pre>`), not when it is a tight item's text
/// or the item is empty.
#[derive(Debug)]
pub struct Item;

impl NodeValue for Item {
    fn render(&self, node: &Node, fmt: &mut dyn Renderer) {
        fmt.open("li", &node.attrs);
        if node
            .children
            .first()
            .is_some_and(|c| is_block(c) || c.is::<Code>())
        {
            fmt.cr();
        }
        fmt.contents(&node.children);
        fmt.close("li");
        fmt.cr();
    }
}

/// A blockquote; an empty one renders as `<blockquote></blockquote>`.
#[derive(Debug)]
pub struct Quote;

impl NodeValue for Quote {
    fn render(&self, node: &Node, fmt: &mut dyn Renderer) {
        fmt.cr();
        fmt.open("blockquote", &node.attrs);
        // Reference definitions render nothing (markdown-it emits no token).
        if node.children.iter().any(|c| !c.is::<Definition>()) {
            fmt.cr();
            fmt.contents(&node.children);
            fmt.cr();
        }
        fmt.close("blockquote");
        fmt.cr();
    }
}

fn rewrite_containers(node: &mut Node) {
    for child in node.children.iter_mut() {
        rewrite_containers(child);
        if child.is::<ListItem>() {
            child.replace(Item);
        } else if child.is::<Blockquote>() {
            child.replace(Quote);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_engine_code_lines(src: &str) -> Vec<bool> {
        let starts = line_starts(src);
        let mut flags = vec![false; starts.len()];
        engine().parse(src).walk(|n, _| {
            if let (true, Some(map)) = (n.is::<CodeFence>() || n.is::<CodeBlock>(), n.srcmap) {
                let (start, end) = map.get_byte_offsets();
                let (a, b) = (line_index(&starts, start), line_index(&starts, end));
                flags[a..=b].iter_mut().for_each(|f| *f = true);
            }
        });
        flags
    }

    #[test]
    fn code_lines_need_no_inline_rules() {
        for src in [
            "```\na\n```\nb",
            "> ```\n> a\n\nb\n\n    c\n    d\n\n  e",
            "- x\n\n      code\n- `y`\n  ~~~\n  z",
            "[r]: /u\n    not code\n\n    code",
            "para *em\n    lazy* continuation\n\n```\nopen to eof",
            "| a |\n|---|\n|    b |\n\n\tcode",
        ] {
            assert_eq!(code_lines(src), full_engine_code_lines(src), "{src:?}");
        }
        assert_eq!(
            code_lines("x\n\n    y\n```\nz\n```"),
            [false, false, true, true, true, true]
        );
    }
}
