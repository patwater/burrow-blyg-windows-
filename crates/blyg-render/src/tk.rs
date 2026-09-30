//! TK instructed-generation scopes, `[TK]instruction[=]output[/TK]` (the
//! Worker's `tk.ts`): the parser, the studio preview strip, and the
//! sentinel technique that wraps generated spans in `blyg-tk-gen` after
//! rendering.
//!
//! Offsets are byte offsets into the working copy (tk.ts uses UTF-16 code
//! units; every offset is only used to slice the same string, so the results
//! are identical).

use regex::Regex;
use std::sync::OnceLock;

use crate::ID_ALPHABET;
use crate::linemap::{MapBuilder, Mapped};
use crate::util::{escape_html, js_trim};

/// One parsed scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TkScope {
    /// Offset of the opening `[TK]`.
    pub start: usize,
    /// Offset just after the closing `[/TK]`.
    pub end: usize,
    /// The instruction, trimmed.
    pub instruction: String,
    /// Text between `[=]` and `[/TK]`; `None` while ungenerated.
    pub output: Option<String>,
    /// De-duplicated `![[id]]` source references anywhere in the scope, in order.
    pub source_ids: Vec<String>,
    /// Offset right after `[=]`, when present.
    pub output_start: Option<usize>,
    /// Alone in its own paragraph: renders as `div.blyg-tk-gen`, not `span`.
    pub block: bool,
}

/// A malformed scope (`nested TK scopes are not supported`,
/// `unterminated scope (missing [/TK])`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TkError {
    pub at: usize,
    pub reason: String,
}

fn source_ref_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(r"!\[\[([{ID_ALPHABET}]{{26}})\]\]")).expect("source ref re")
    })
}

fn extract_source_ids(scope_text: &str) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for c in source_ref_re().captures_iter(scope_text) {
        let id = &c[1];
        if !ids.iter().any(|x| x == id) {
            ids.push(id.to_string());
        }
    }
    ids
}

/// `BLANK_BEFORE = /(^|\n[ \t]*\n)[ \t]*$/` on the text before the scope.
fn blank_before(before: &str) -> bool {
    let t = before.trim_end_matches([' ', '\t']);
    if t.is_empty() {
        return true;
    }
    let Some(t) = t.strip_suffix('\n') else {
        return false;
    };
    let t = t.trim_end_matches([' ', '\t']);
    t.ends_with('\n')
}

/// `BLANK_AFTER = /^[ \t]*(\n[ \t]*\n|$)/` on the text after the scope.
fn blank_after(after: &str) -> bool {
    let t = after.trim_start_matches([' ', '\t']);
    if t.is_empty() {
        return true;
    }
    let Some(t) = t.strip_prefix('\n') else {
        return false;
    };
    t.trim_start_matches([' ', '\t']).starts_with('\n')
}

/// Linear token scan, exactly as tk.ts `parseScopes`: no bracket balancing,
/// no nesting (a nested `[TK]` skips the whole outer scope), and an
/// unterminated scope stops the scan.
pub fn parse_scopes(md: &str) -> (Vec<TkScope>, Vec<TkError>) {
    let mut scopes = Vec::new();
    let mut errors = Vec::new();
    let mut i = 0;
    while let Some(tk) = md[i..].find("[TK]").map(|p| p + i) {
        let Some(close) = md[tk + 4..].find("[/TK]").map(|p| p + tk + 4) else {
            errors.push(TkError {
                at: tk,
                reason: "unterminated scope (missing [/TK])".into(),
            });
            break;
        };
        if let Some(nested) = md[tk + 4..].find("[TK]").map(|p| p + tk + 4)
            && nested < close
        {
            errors.push(TkError {
                at: nested,
                reason: "nested TK scopes are not supported".into(),
            });
            i = close + 5;
            continue;
        }
        let eq = md[tk + 4..]
            .find("[=]")
            .map(|p| p + tk + 4)
            .filter(|&e| e < close);
        let instr_end = eq.unwrap_or(close);
        let end = close + 5;
        scopes.push(TkScope {
            start: tk,
            end,
            instruction: js_trim(&md[tk + 4..instr_end]).to_string(),
            output: eq.map(|e| md[e + 3..close].to_string()),
            source_ids: extract_source_ids(&md[tk..end]),
            output_start: eq.map(|e| e + 3),
            block: blank_before(&md[..tk]) && blank_after(&md[end..]),
        });
        i = end;
    }
    (scopes, errors)
}

/// A span of preview text standing in for a scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Span {
    pub start: usize,
    pub end: usize,
    pub block: bool,
}

/// tk.ts `previewStrip`: every scope becomes its output, or the visible
/// `⚠ ungenerated — <instruction>` placeholder.
pub(crate) fn preview_strip(src: &Mapped, scopes: &[TkScope]) -> (Mapped, Vec<Span>) {
    let mut b = MapBuilder::new(src);
    let mut spans = Vec::with_capacity(scopes.len());
    let mut last = 0;
    let mut out_len = 0;
    for s in scopes {
        b.copy(last, s.start);
        out_len += s.start - last;
        let span_start = out_len;
        match (&s.output, s.output_start) {
            (Some(out), Some(os)) => {
                b.copy(os, os + out.len());
                out_len += out.len();
            }
            _ => {
                let instr = if s.instruction.is_empty() {
                    "(no instruction)"
                } else {
                    s.instruction.as_str()
                };
                let text = format!("⚠ ungenerated — {instr}");
                b.insert(&text, src.line_at(s.start));
                out_len += text.len();
            }
        }
        spans.push(Span {
            start: span_start,
            end: out_len,
            block: s.block,
        });
        last = s.end;
    }
    b.copy(last, src.text.len());
    (b.finish(), spans)
}

// C0 controls, as tk.ts uses them since the Worker's patch 9: they end a
// linkify match, an autolink and a link destination alike, so a sentinel
// never lands in an href.
pub(crate) const BLOCK_SENTINEL: char = '\u{1}';
pub(crate) const INLINE_OPEN: char = '\u{2}';
pub(crate) const INLINE_CLOSE: char = '\u{3}';

fn is_sentinel(c: char) -> bool {
    matches!(c, '\u{1}'..='\u{3}')
}

/// The Worker's `annotateGenerated` with every span highlighted (studio
/// preview): block spans become a one-line placeholder token whose
/// independently rendered HTML is spliced in later; inline spans are wrapped
/// in sentinels so Markdown still parses across their boundaries.
pub(crate) struct Annotated {
    pub doc: Mapped,
    /// (token, block span text, source line of the span)
    pub blocks: Vec<(String, String, usize)>,
    /// `[start, end)` offsets in `doc.text` of every generated span (the
    /// Worker's `inertRanges`): an own-line `![[id]]` inside one is a TK
    /// source ref, never a quote.
    pub inert: Vec<(usize, usize)>,
    pub has_inline: bool,
}

pub(crate) fn annotate(stripped: &Mapped, spans: &[Span]) -> Annotated {
    let mut b = MapBuilder::new(stripped);
    let mut blocks = Vec::new();
    let mut inert = Vec::with_capacity(spans.len());
    let mut has_inline = false;
    let mut last = 0;
    for (i, span) in spans.iter().enumerate() {
        b.copy(last, span.start);
        let line = stripped.line_at(span.start);
        let inert_start = b.len();
        if span.block {
            let token = format!("{BLOCK_SENTINEL}{i}{BLOCK_SENTINEL}");
            b.insert(&token, line);
            blocks.push((token, stripped.text[span.start..span.end].to_string(), line));
        } else {
            has_inline = true;
            b.insert(&INLINE_OPEN.to_string(), line);
            b.copy(span.start, span.end);
            b.insert(&INLINE_CLOSE.to_string(), line);
        }
        inert.push((inert_start, b.len()));
        last = span.end;
    }
    b.copy(last, stripped.text.len());
    Annotated {
        doc: b.finish(),
        blocks,
        inert,
        has_inline,
    }
}

/// A rendered block span, for [`apply_wrappers`].
pub(crate) struct Block {
    pub token: String,
    /// The complete `<div class="blyg-tk-gen">…</div>`.
    pub html: String,
    /// The span's raw text, spliced in (escaped) where Markdown put the token
    /// inside code.
    pub literal: String,
}

/// The Worker's `applyGeneratedWrappers`. When top-level blocks carry
/// `data-line`, the placeholder paragraph is `<p data-line="N">token</p>` and
/// the attribute moves onto the `<div>`.
pub(crate) fn apply_wrappers(html: String, blocks: &[Block], has_inline: bool) -> String {
    // Every step below only touches sentinels.
    if !html.contains(is_sentinel) {
        return html;
    }
    let mut out = html;
    for b in blocks {
        // Spliced literally: generated text's `$&` stays `$&`.
        let plain = format!("<p>{}</p>", b.token);
        if let Some(at) = out.find(&plain) {
            out.replace_range(at..at + plain.len(), &b.html);
            continue;
        }
        // `<p data-line="N">token</p>`
        let tail = format!(">{}</p>", b.token);
        let Some(t) = out.find(&tail) else { continue };
        let Some(p) = out[..t].rfind("<p ") else {
            continue;
        };
        let attrs = &out[p + 2..t];
        if !attrs.starts_with(" data-line=\"") || attrs.contains('<') {
            continue;
        }
        let with_attrs = b.html.replacen(
            "<div class=\"blyg-tk-gen\"",
            &format!("<div class=\"blyg-tk-gen\"{attrs}"),
            1,
        );
        out.replace_range(p..t + tail.len(), &with_attrs);
    }
    // A sentinel inside a tag (image alt) must not become markup: drop it.
    // This is `/<[^>]*>/g`: from each `<` to the next `>`.
    if out.contains(is_sentinel) {
        let mut cleaned = String::with_capacity(out.len());
        let mut rest = out.as_str();
        while let Some(lt) = rest.find('<') {
            let Some(gt) = rest[lt..].find('>').map(|g| g + lt) else {
                break;
            };
            cleaned.push_str(&rest[..lt]);
            cleaned.extend(rest[lt..=gt].chars().filter(|&c| !is_sentinel(c)));
            rest = &rest[gt + 1..];
        }
        cleaned.push_str(rest);
        out = cleaned;
    }
    // A block token Markdown did not leave in its own <p> sits in code, which
    // is literal: it becomes the span's escaped text.
    for b in blocks {
        if out.contains(&b.token) {
            let span = format!(
                "<span class=\"blyg-tk-gen\">{}</span>",
                escape_html(&b.literal)
            );
            out = out.replace(&b.token, &span);
        }
    }
    if has_inline {
        out = out
            .replace(INLINE_OPEN, "<span class=\"blyg-tk-gen\">")
            .replace(INLINE_CLOSE, "</span>");
    }
    // No marker ever ships.
    out.retain(|c| !is_sentinel(c));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_like_tk_ts() {
        let (s, e) = parse_scopes("a [TK]x[=]y[/TK] b");
        assert!(e.is_empty());
        assert_eq!(s[0].instruction, "x");
        assert_eq!(s[0].output.as_deref(), Some("y"));
        assert!(!s[0].block);

        let (s, _) = parse_scopes("[TK] do it [/TK]\n\nnext");
        assert!(s[0].block);
        assert_eq!(s[0].output, None);

        let (s, e) = parse_scopes("A [TK]o [TK]i[/TK] t[/TK] B");
        assert!(s.is_empty() || s.len() == 1);
        assert_eq!(e[0].reason, "nested TK scopes are not supported");

        let (_, e) = parse_scopes("x [TK]open");
        assert_eq!(e[0].reason, "unterminated scope (missing [/TK])");
    }
}
