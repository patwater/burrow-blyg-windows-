//! Transclusion `![[id]]` (threads only): the directive grammar, resolution
//! through a [`Resolver`], the baked `blockquote.blyg-transclusion` markup
//! and its provenance line (the Worker's `transclusion.ts`
//! `previewTransclusions` and `pages.ts` `transclusionProvenance` +
//! `injectProvenance`).

use regex::Regex;
use std::fmt;
use std::sync::OnceLock;

use crate::embeds::JS_WS;
use crate::linemap::Mapped;
use crate::markdown::{self, MdStats};
use crate::util::{escape_html, js_trim};
use crate::{ID_ALPHABET, ItemKind};

/// A resolved quote: the snapshot to bake and the provenance to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The source blyg's origin (e.g. `https://blyg.example.com/`) for an
    /// imported item; `None` for your own.
    pub origin: Option<String>,
    pub id: String,
    /// The version whose HTML is baked (for a retained tombstone, the pinned one).
    pub version: u32,
    /// The target's authored kind (threads nest).
    pub kind: ItemKind,
    /// The snapshot's stored `content_html`, inserted verbatim. Resolvers must
    /// supply HTML that was sanitised when it was stored (as the reference
    /// importer does); the preview page's CSP is the backstop.
    pub content_html: String,
    /// For an imported item: the source blyg's display name (the provenance
    /// line reads "from <em>name</em>"; without it, the origin's host).
    pub author: Option<String>,
    /// For an imported item: the origin's own `page` path, when it declared one.
    pub page: Option<String>,
}

/// What a [`Resolver`] says about an id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Found(Found),
    /// No local item and no imported blyg item with this id.
    NotFound,
    /// Imported from more than one origin.
    Ambiguous,
    /// Imported from a plain RSS (L0) feed, which is not quotable.
    RssNotQuotable,
    /// `![[id@vN]]`: reserved. The renderer detects these itself and never
    /// asks; a resolver may return it for completeness.
    ReservedVersion,
    /// Any other reason it cannot be quoted (draft, withdrawn, circular, …).
    Unavailable(UnresolvedReason),
}

/// Resolves quote ids from the local store (your published items and your
/// imported reading items). Publishing never fetches, and neither does this.
pub trait Resolver {
    fn resolve(&self, id: &str) -> Resolution;
}

/// A resolver that knows nothing (every quote is unresolved).
pub struct NoResolver;

impl Resolver for NoResolver {
    fn resolve(&self, _id: &str) -> Resolution {
        Resolution::NotFound
    }
}

/// Why a directive did not resolve. `Display` gives the Worker's exact reason
/// string (what the unresolved marker shows); [`UnresolvedReason::human`]
/// gives a sentence for status bars.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnresolvedReason {
    UnknownItem,
    Draft,
    Withdrawn,
    Ambiguous,
    SourceWithdrawn,
    RssNotQuotable,
    SelfQuote,
    Circular,
    ReservedVersion,
    Other(String),
}

impl fmt::Display for UnresolvedReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            UnresolvedReason::UnknownItem => "unknown item",
            UnresolvedReason::Draft => "item is a draft, not published",
            UnresolvedReason::Withdrawn => "item is withdrawn",
            UnresolvedReason::Ambiguous => "ambiguous id: imported from more than one origin",
            UnresolvedReason::SourceWithdrawn => "source withdrawn by origin",
            UnresolvedReason::RssNotQuotable => "source is a plain RSS (L0) item, not a blyg item",
            UnresolvedReason::SelfQuote => "a thread cannot transclude itself",
            UnresolvedReason::Circular => {
                "circular transclusion: that thread already quotes this one"
            }
            UnresolvedReason::ReservedVersion => {
                "explicit-version references (@vN) are reserved, not supported in v0.1"
            }
            UnresolvedReason::Other(s) => s,
        };
        f.write_str(s)
    }
}

impl UnresolvedReason {
    /// A plain-language explanation, for the status bar and publish warnings.
    pub fn human(&self) -> String {
        match self {
            UnresolvedReason::UnknownItem => "it isn't in your posts or your reading list".into(),
            UnresolvedReason::Draft => "that post is still a draft".into(),
            UnresolvedReason::Withdrawn => "that post has been withdrawn".into(),
            UnresolvedReason::Ambiguous => {
                "more than one blyg you read has an item with this id".into()
            }
            UnresolvedReason::SourceWithdrawn => "its author withdrew it".into(),
            UnresolvedReason::RssNotQuotable => {
                "it comes from a plain RSS feed, which can't be quoted".into()
            }
            UnresolvedReason::SelfQuote => "a thread can't quote itself".into(),
            UnresolvedReason::Circular => "that thread already quotes this one".into(),
            UnresolvedReason::ReservedVersion => {
                "quoting a specific version (@vN) isn't supported yet".into()
            }
            UnresolvedReason::Other(s) => s.clone(),
        }
    }
}

/// A directive that did not resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unresolved {
    /// Source line (0-based).
    pub line: usize,
    /// The directive as written, trimmed.
    pub directive: String,
    pub reason: UnresolvedReason,
}

/// A resolved quote, in directive order (the wire's `transclusions[]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quote {
    pub id: String,
    pub version: u32,
    pub origin: Option<String>,
    /// Source line (0-based).
    pub line: usize,
}

fn directive_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(
            r"^[{JS_WS}]*!\[\[([{ID_ALPHABET}]{{26}})\]\][{JS_WS}]*$"
        ))
        .expect("directive re")
    })
}

fn reserved_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(
            r"^[{JS_WS}]*!\[\[([{ID_ALPHABET}]{{26}})@v[0-9]+\]\][{JS_WS}]*$"
        ))
        .expect("reserved re")
    })
}

pub(crate) struct WalkResult {
    pub html: String,
    pub quotes: Vec<(Quote, Found)>,
    pub unresolved: Vec<Unresolved>,
    pub md: MdStats,
    /// Source line of each top-level block, in document order.
    pub block_lines: Vec<usize>,
}

fn data_line(on: bool, line: usize) -> String {
    if on {
        format!(" data-line=\"{line}\"")
    } else {
        String::new()
    }
}

/// transclusion.ts `literalLines`: per line, whether it can never be a
/// directive. A line is literal when Markdown renders it as code, or when it
/// overlaps a generated span (`inert`, `[start, end)` offsets into `text`).
fn literal_lines(text: &str, lines: &[&str], inert: &[(usize, usize)]) -> Vec<bool> {
    // Only a line holding `![[` could be a directive; skip the extra parse.
    if !text.contains("![[") {
        return vec![false; lines.len()];
    }
    let mut flags = markdown::code_lines(text);
    let mut start = 0;
    for (flag, line) in flags.iter_mut().zip(lines) {
        let end = start + line.len();
        *flag = *flag || inert.iter().any(|&(a, b)| start < b && end > a);
        start = end + 1;
    }
    flags
}

/// transclusion.ts `walk` with the preview's unresolved placeholder. Lines
/// in code or inside a generated span (`inert`) stay prose.
pub(crate) fn walk(
    doc: &Mapped,
    inert: &[(usize, usize)],
    resolver: &dyn Resolver,
    self_id: Option<&str>,
    lines_on: bool,
) -> WalkResult {
    let mut parts: Vec<String> = Vec::new();
    let mut quotes = Vec::new();
    let mut unresolved = Vec::new();
    let mut md = MdStats::default();
    let mut block_lines = Vec::new();
    let mut prose_start: Option<usize> = None; // first line index of pending prose
    let lines: Vec<&str> = doc.text.split('\n').collect();
    let literal = literal_lines(&doc.text, &lines, inert);

    let flush = |from: Option<usize>,
                 to: usize,
                 parts: &mut Vec<String>,
                 md: &mut MdStats,
                 block_lines: &mut Vec<usize>| {
        if let Some(from) = from {
            let chunk = lines[from..to].join("\n");
            let map = |j: usize| doc.lines[from + j];
            let (html, stats) = markdown::render(&chunk, if lines_on { Some(&map) } else { None });
            md.images += stats.images;
            md.videos += stats.videos;
            block_lines.extend(stats.block_lines);
            parts.push(html);
        }
    };

    for (k, line) in lines.iter().enumerate() {
        let src_line = doc.lines[k];
        let fail = |reason: UnresolvedReason,
                    parts: &mut Vec<String>,
                    unresolved: &mut Vec<Unresolved>,
                    block_lines: &mut Vec<usize>| {
            parts.push(format!(
                "<blockquote class=\"blyg-transclusion unresolved\"{}><p>⚠ unresolvable: {}</p></blockquote>",
                data_line(lines_on, src_line),
                escape_html(&reason.to_string())
            ));
            if lines_on {
                block_lines.push(src_line);
            }
            unresolved.push(Unresolved {
                line: src_line,
                directive: js_trim(line).to_string(),
                reason,
            });
        };
        if literal[k] {
            prose_start.get_or_insert(k);
            continue;
        }
        if reserved_re().is_match(line) {
            flush(prose_start.take(), k, &mut parts, &mut md, &mut block_lines);
            fail(
                UnresolvedReason::ReservedVersion,
                &mut parts,
                &mut unresolved,
                &mut block_lines,
            );
            continue;
        }
        let Some(c) = directive_re().captures(line) else {
            prose_start.get_or_insert(k);
            continue;
        };
        flush(prose_start.take(), k, &mut parts, &mut md, &mut block_lines);
        let id = &c[1];
        let found = match resolver.resolve(id) {
            Resolution::Found(f) => {
                if f.origin.is_none()
                    && f.kind == ItemKind::Thread
                    && self_id == Some(f.id.as_str())
                {
                    Err(UnresolvedReason::SelfQuote)
                } else {
                    Ok(f)
                }
            }
            Resolution::NotFound => Err(UnresolvedReason::UnknownItem),
            Resolution::Ambiguous => Err(UnresolvedReason::Ambiguous),
            Resolution::RssNotQuotable => Err(UnresolvedReason::RssNotQuotable),
            Resolution::ReservedVersion => Err(UnresolvedReason::ReservedVersion),
            Resolution::Unavailable(r) => Err(r),
        };
        match found {
            Err(reason) => fail(reason, &mut parts, &mut unresolved, &mut block_lines),
            Ok(f) => {
                let origin_attr = f
                    .origin
                    .as_ref()
                    .map(|o| format!(" data-blyg-origin=\"{}\"", escape_html(o)))
                    .unwrap_or_default();
                parts.push(format!(
                    "<blockquote class=\"blyg-transclusion\" data-blyg-id=\"{}\" data-blyg-version=\"{}\"{origin_attr}{}>\n{}\n</blockquote>",
                    escape_html(&f.id),
                    f.version,
                    data_line(lines_on, src_line),
                    f.content_html
                ));
                if lines_on {
                    block_lines.push(src_line);
                }
                quotes.push((
                    Quote {
                        id: f.id.clone(),
                        version: f.version,
                        origin: f.origin.clone(),
                        line: src_line,
                    },
                    f,
                ));
            }
        }
    }
    flush(
        prose_start.take(),
        lines.len(),
        &mut parts,
        &mut md,
        &mut block_lines,
    );
    WalkResult {
        html: parts.join("\n"),
        quotes,
        unresolved,
        md,
        block_lines,
    }
}

/// `new URL(origin).host`, for the provenance label.
fn host_of(origin: &str) -> String {
    let rest = origin.split_once("://").map_or(origin, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = host.to_ascii_lowercase();
    let scheme = origin
        .split_once("://")
        .map(|(s, _)| s.to_ascii_lowercase());
    match (scheme.as_deref(), host.rsplit_once(':')) {
        (Some("https"), Some((h, "443"))) | (Some("http"), Some((h, "80"))) => h.to_string(),
        _ => host,
    }
}

/// pages.ts `transclusionProvenance` for one direct quote.
pub(crate) fn provenance_line(f: &Found, mount: &str) -> String {
    let (href, label) = match &f.origin {
        Some(origin) => {
            // importer/util.ts blygItemUrl
            let href = match f.page.as_deref().filter(|p| !p.is_empty()) {
                Some(page) => format!("{origin}{}", page.strip_prefix('/').unwrap_or(page)),
                None => format!(
                    "{origin}{}/{}/",
                    if f.kind == ItemKind::Thread { "t" } else { "f" },
                    f.id
                ),
            };
            let label = match f.author.as_deref().filter(|a| !a.is_empty()) {
                Some(name) => format!("from <em>{}</em> ↗", escape_html(name)),
                None => format!("from {} ↗", escape_html(&host_of(origin))),
            };
            (href, label)
        }
        None => {
            let seg = if f.kind == ItemKind::Thread { "t" } else { "f" };
            (
                format!("{mount}/{seg}/{}/", f.id),
                format!("{} ↗", f.kind.as_str()),
            )
        }
    };
    format!(
        "<p class=\"provenance\"><a href=\"{}\">{label}</a> · snapshot of v{}</p>",
        escape_html(&href),
        f.version
    )
}

/// pages.ts `injectProvenance`: one provenance paragraph inside each
/// top-level `blockquote.blyg-transclusion`, in order; nested quotes get none.
pub(crate) fn inject_provenance(html: &str, provenance: &[String]) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"<blockquote\b[^>]*>|</blockquote>").expect("bq re"));
    let mut out =
        String::with_capacity(html.len() + provenance.iter().map(|p| p.len() + 1).sum::<usize>());
    let mut last = 0;
    let mut depth = 0usize;
    let mut in_transclusion = false;
    let mut i = 0;
    for m in re.find_iter(html) {
        if m.as_str().starts_with("</") {
            depth = depth.saturating_sub(1);
            if depth == 0 && in_transclusion {
                out.push_str(&html[last..m.start()]);
                if let Some(p) = provenance.get(i) {
                    out.push('\n');
                    out.push_str(p);
                }
                i += 1;
                last = m.start();
                in_transclusion = false;
            }
        } else {
            if depth == 0 {
                in_transclusion = m.as_str().contains("class=\"blyg-transclusion\"");
            }
            depth += 1;
        }
    }
    out.push_str(&html[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grammar() {
        assert!(directive_re().is_match("  ![[01j9zq3k4m5n6p7q8r9s0t1v2w]]\u{A0}"));
        assert!(!directive_re().is_match("![[01J9ZQ3K4M5N6P7Q8R9S0T1V2W]]"));
        assert!(!directive_re().is_match("see ![[01j9zq3k4m5n6p7q8r9s0t1v2w]]"));
        assert!(reserved_re().is_match("![[01j9zq3k4m5n6p7q8r9s0t1v2w@v12]]"));
        assert_eq!(
            host_of("https://Blyg.Example.com:443/x/"),
            "blyg.example.com"
        );
    }

    #[test]
    fn provenance_depth() {
        let html = "<blockquote class=\"blyg-transclusion\" data-blyg-id=\"a\">\n<blockquote class=\"blyg-transclusion\">in</blockquote>\n</blockquote><blockquote>plain</blockquote>";
        let out = inject_provenance(html, &["<p>P</p>".to_string()]);
        assert_eq!(out.matches("<p>P</p>").count(), 1);
        assert!(out.contains("in</blockquote>\n\n<p>P</p></blockquote>"));
    }
}
