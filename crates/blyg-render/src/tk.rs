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
use crate::markdown;
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
    /// Set for an `impyrt` scope: text generated elsewhere and pasted in.
    pub imported: Option<Imported>,
}

/// An `impyrt` scope's provenance: the model, only when the author named one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Imported {
    pub model: Option<String>,
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

/// `IMPYRT_INLINE = /^\s*impyrt(?:[ \t]+([^\s=]+))?[ \t]*=/i`
fn impyrt_inline() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(
            r"^(?i)[{ws}]*impyrt(?:[ \t]+([^{ws}=]+))?[ \t]*=",
            ws = crate::embeds::JS_WS
        ))
        .expect("impyrt inline re")
    })
}

/// `IMPYRT_INSTRUCTION = /^impyrt(?:[ \t]+(\S+))?$/i`
fn impyrt_instruction() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(
            r"^(?i)impyrt(?:[ \t]+([^{ws}]+))?$",
            ws = crate::embeds::JS_WS
        ))
        .expect("impyrt instruction re")
    })
}

/// Linear token scan, exactly as tk.ts `parseScopes`: no bracket balancing,
/// no nesting (a nested `[TK]` skips the whole outer scope), and an
/// unterminated scope stops the scan. A `[TK]` inside code is an example of
/// the grammar, not a scope (studio#3). `[TK]impyrt=…[/TK]` and
/// `[TK]impyrt <model>=…[/TK]` (or the same with `[=]`) mark pasted
/// generated text (decision #37): its instruction reads `impyrt`, and it
/// declares no sources.
pub fn parse_scopes(md: &str) -> (Vec<TkScope>, Vec<TkError>) {
    let mut scopes = Vec::new();
    let mut errors = Vec::new();
    let code = if md.contains("[TK]") {
        markdown::code_ranges(md)
    } else {
        Vec::new()
    };
    let mut i = 0;
    while let Some(tk) = md[i..].find("[TK]").map(|p| p + i) {
        if markdown::in_ranges(&code, tk) {
            i = tk + 4;
            continue;
        }
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
        let mut instruction = js_trim(&md[tk + 4..instr_end]).to_string();
        let mut output_start = eq.map(|e| e + 3);
        let mut imported = None;
        if eq.is_none() {
            if let Some(c) = impyrt_inline().captures(&md[tk + 4..close]) {
                output_start = Some(tk + 4 + c[0].len());
                instruction = "impyrt".into();
                imported = Some(Imported {
                    model: c.get(1).map(|m| m.as_str().to_string()),
                });
            }
        } else if let Some(c) = impyrt_instruction().captures(&instruction) {
            imported = Some(Imported {
                model: c.get(1).map(|m| m.as_str().to_string()),
            });
            instruction = "impyrt".into();
        }
        scopes.push(TkScope {
            start: tk,
            end,
            instruction,
            output: output_start.map(|o| md[o..close].to_string()),
            source_ids: if imported.is_some() {
                Vec::new()
            } else {
                // The instruction's own refs (studio 0.32.2, upstream #60): a
                // directive the output adds is not a source.
                extract_source_ids(&md[tk + 4..instr_end])
            },
            output_start,
            block: blank_before(&md[..tk]) && blank_after(&md[end..]),
            imported,
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

// Private Use Area characters, as tk.ts uses them (U+E000–U+E002). Markdown
// treats them as ordinary text, so linkify and link destinations take them
// as URL characters: `apply_wrappers` drops their percent-encoded form from
// an `href`, and drops them inside any tag.
pub(crate) const BLOCK_SENTINEL: char = '\u{E000}';
pub(crate) const INLINE_OPEN: char = '\u{E001}';
pub(crate) const INLINE_CLOSE: char = '\u{E002}';
/// `encodeURIComponent` of the inline sentinels.
const ENCODED_INLINE: [&str; 2] = ["%EE%80%81", "%EE%80%82"];

fn is_sentinel(c: char) -> bool {
    matches!(c, '\u{E000}'..='\u{E002}')
}

/// The Worker's `annotateGenerated` with every span highlighted (studio
/// preview): block spans become a one-line placeholder token whose
/// independently rendered HTML is spliced in later; inline spans are wrapped
/// in sentinels so Markdown still parses across their boundaries.
pub(crate) struct Annotated {
    pub doc: Mapped,
    /// (token, block span text, source line of the span)
    pub blocks: Vec<(String, String, usize)>,
    pub has_inline: bool,
}

pub(crate) fn annotate(stripped: &Mapped, spans: &[Span]) -> Annotated {
    let mut b = MapBuilder::new(stripped);
    let mut blocks = Vec::new();
    let mut has_inline = false;
    let mut last = 0;
    for (i, span) in spans.iter().enumerate() {
        b.copy(last, span.start);
        let line = stripped.line_at(span.start);
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
        last = span.end;
    }
    b.copy(last, stripped.text.len());
    Annotated {
        doc: b.finish(),
        blocks,
        has_inline,
    }
}

/// A rendered block span, for [`apply_wrappers`].
pub(crate) struct Block {
    pub token: String,
    /// The complete `<div class="blyg-tk-gen">…</div>`.
    pub html: String,
    /// The span's raw text, spliced in (escaped) if Markdown did not leave
    /// the token in a paragraph of its own.
    pub literal: String,
}

/// The Worker's `applyGeneratedWrappers`. When top-level blocks carry
/// `data-line`, the placeholder paragraph is `<p data-line="N">token</p>` and
/// the attribute moves onto the `<div>`.
pub(crate) fn apply_wrappers(html: String, blocks: &[Block], has_inline: bool) -> String {
    // Every step below only touches sentinels.
    if !html.contains(is_sentinel) && !(has_inline && html.contains("%EE%80%8")) {
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
    if has_inline {
        // Markers linkify or a link destination took into a URL were
        // percent-encoded there: drop them, the URL is the generated text.
        for enc in ENCODED_INLINE {
            out = out.replace(enc, "");
        }
        // `/(<[^>]*>)/`: in a tag (an image's alt) a marker is dropped; in
        // text it becomes the span.
        let mut wrapped = String::with_capacity(out.len() + 64);
        let mut rest = out.as_str();
        let text = |t: &str, w: &mut String| {
            for c in t.chars() {
                match c {
                    INLINE_OPEN => w.push_str("<span class=\"blyg-tk-gen\">"),
                    INLINE_CLOSE => w.push_str("</span>"),
                    c => w.push(c),
                }
            }
        };
        while let Some(lt) = rest.find('<') {
            let Some(gt) = rest[lt..].find('>').map(|g| g + lt) else {
                break;
            };
            text(&rest[..lt], &mut wrapped);
            wrapped.extend(
                rest[lt..=gt]
                    .chars()
                    .filter(|&c| c != INLINE_OPEN && c != INLINE_CLOSE),
            );
            rest = &rest[gt + 1..];
        }
        text(rest, &mut wrapped);
        out = wrapped;
    }
    // Not in the Worker, which leaves these in: a block token Markdown did
    // not leave in a paragraph of its own becomes the span's escaped text,
    // and no other marker ever ships.
    for b in blocks {
        if out.contains(&b.token) {
            let span = format!(
                "<span class=\"blyg-tk-gen\">{}</span>",
                escape_html(&b.literal)
            );
            out = out.replace(&b.token, &span);
        }
    }
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
