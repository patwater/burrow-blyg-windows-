//! The stub editor's passage chooser: a port of the reference studio's
//! `src/ui/stub-quote.ts` (studio 0.31). A stub opens quoting the whole
//! target; choosing a passage writes §10.1's partial grammar under the
//! directive (`>` lines), and "quote whole post" removes it again. Pure text
//! in, text out.
//!
//! The quote is found the way publish finds it: the first own-line
//! `![[id]]` that isn't code ([`literal_lines`], the publish walker's rule),
//! and the run of `>` lines directly under it ([`attached_quote`]), so the
//! chooser never edits a different quote from the one publish checks.
//!
//! Offsets (`at`) are byte offsets into the text, where the TypeScript has
//! UTF-16 indices: the editor's caret is a byte offset here.
//!
//! [`literal_lines`]: crate::transclusion
//! [`attached_quote`]: crate::transclusion

use crate::links::normalize_selection;
use crate::transclusion::{attached_quote, directive_id, is_quote_line, literal_lines};

/// What a stub's body does with its target (`stubQuoteForm`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StubQuoteForm {
    /// `![[id]]` alone: the whole post is quoted.
    Whole,
    /// `![[id]]` with `>` lines under it: a passage.
    Passage,
}

/// A normalized selection as an attached markdown blockquote: `>` on every
/// line, a bare `>` between blocks (markdown.ts `quoteLines`).
pub fn quote_lines(selection: &str) -> String {
    selection
        .split('\n')
        .map(|l| format!("> {l}"))
        .collect::<Vec<_>>()
        .join("\n>\n")
}

/// Byte offset at which each line starts (`lineOffsets`).
fn line_offsets(lines: &[&str]) -> Vec<usize> {
    let mut at = 0;
    lines
        .iter()
        .map(|l| {
            let s = at;
            at += l.len() + 1;
            s
        })
        .collect()
}

/// `line` is an own-line `![[id]]` of `id` (`DIRECTIVE_LINE`). An id outside
/// the blyg alphabet (the sample data's) matches its own line too: no real
/// target has one, so publish's rule is unchanged for every real stub.
fn directive_of(line: &str, id: &str) -> bool {
    directive_id(line) == Some(id) || crate::util::js_trim(line) == format!("![[{id}]]")
}

/// Every own-line directive of `id` outside code: (line, attached quote?, next line).
fn quotes_of(md: &str, id: &str) -> Vec<(usize, bool, usize)> {
    let lines: Vec<&str> = md.split('\n').collect();
    let literal = literal_lines(md, &lines);
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if literal[i] || !directive_of(line, id) {
            continue;
        }
        let (quote, next) = attached_quote(&lines, i);
        out.push((i, quote.is_some(), next));
    }
    out
}

/// `Whole` or `Passage` when the body quotes `id`, `None` when the author
/// removed the quote (a response by link).
pub fn stub_quote_form(md: &str, id: &str) -> Option<StubQuoteForm> {
    quotes_of(md, id).first().map(|&(_, partial, _)| {
        if partial {
            StubQuoteForm::Passage
        } else {
            StubQuoteForm::Whole
        }
    })
}

/// How many partial quotes of `id` the body holds. More than one is a
/// running commentary, which the chooser must not collapse.
pub fn passage_count(md: &str, id: &str) -> usize {
    quotes_of(md, id).iter().filter(|q| q.1).count()
}

/// Another partial quote of `id`, inserted as its own block after the line
/// holding `at` (the editor's caret), or at the end when the caret is at the
/// very start or inside a quote's own lines: never splitting a quote.
pub fn add_stub_quote(md: &str, id: &str, passage: &str, at: usize) -> String {
    let selection = normalize_selection(passage);
    if selection.is_empty() {
        return md.to_string();
    }
    let block = format!("![[{id}]]\n{}", quote_lines(&selection));
    let lines: Vec<&str> = md.split('\n').collect();
    let starts = line_offsets(&lines);
    let last = lines.len() - 1;
    let mut line = if at == 0 {
        last
    } else {
        (0..lines.len())
            .find(|&i| i == last || starts[i + 1] > at)
            .unwrap_or(last)
    };
    // Step past a quote the caret sits in (its `>` lines or its directive).
    while line + 1 < lines.len() && is_quote_line(lines[line + 1]) {
        line += 1;
    }
    let before = lines[..=line].join("\n");
    let before = before.trim_end_matches('\n');
    let after = lines[line + 1..].join("\n");
    let after = after.trim_start_matches('\n');
    let out = format!("{before}\n\n{block}\n\n{after}");
    let trimmed = out.trim_end_matches('\n');
    if trimmed.len() == out.len() {
        out
    } else {
        format!("{trimmed}\n\n")
    }
}

/// The body with its quote of `id` set to `passage` (a selection), or to the
/// whole post when `passage` is `None`. A body with no quote of `id` gains
/// one at the top, where the stub action puts it.
pub fn with_stub_quote(md: &str, id: &str, passage: Option<&str>) -> String {
    let selection = passage.map(normalize_selection).unwrap_or_default();
    let mut block = format!("![[{id}]]");
    if !selection.is_empty() {
        block.push('\n');
        block.push_str(&quote_lines(&selection));
    }
    match quotes_of(md, id).first() {
        None => format!("{block}\n\n{}", md.trim_start_matches('\n')),
        Some(&(at, _, next)) => {
            let lines: Vec<&str> = md.split('\n').collect();
            let mut out: Vec<&str> = lines[..at].to_vec();
            out.push(&block);
            out.extend_from_slice(&lines[next..]);
            out.join("\n")
        }
    }
}

/// The body a new stub of `id` starts with: the whole post quoted, the
/// caret below it (studio 0.31's `POST /api/items {mode: "response"}`).
pub fn stub_body(id: &str) -> String {
    format!("![[{id}]]\n\n")
}

/// The hint line naming what a stub's draft does with its target (studio
/// 0.31's stub editor).
pub fn stub_hint(md: &str, id: &str) -> String {
    let passages = passage_count(md, id);
    match stub_quote_form(md, id) {
        Some(StubQuoteForm::Whole) => "Quoting the whole post. Write above the quote for a quote post, below it for a reply, or nothing for a repost.".into(),
        _ if passages > 1 => format!(
            "Quoting {passages} passages: a running commentary. Choose another to add it after the cursor."
        ),
        Some(StubQuoteForm::Passage) => {
            "Quoting a passage: the > lines under the quote. Write above it, below it, or both.".into()
        }
        None => "Not quoting the post: this is a response by link.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // stub-quote.test.ts, case for case.
    const ID: &str = "7c9wk2mhq0v3xj8tn5rzfd41bg";
    const OTHER: &str = "1vgtgz0gq5b2c9k7d3m8r4n6xy";

    #[test]
    fn a_passage_goes_under_the_directive_and_back_out_again() {
        let whole = format!("![[{ID}]]\n\nMy reply.");
        assert_eq!(stub_quote_form(&whole, ID), Some(StubQuoteForm::Whole));
        let partial = with_stub_quote(&whole, ID, Some("First paragraph.\nSecond one."));
        assert_eq!(
            partial,
            format!("![[{ID}]]\n> First paragraph.\n>\n> Second one.\n\nMy reply.")
        );
        assert_eq!(stub_quote_form(&partial, ID), Some(StubQuoteForm::Passage));
        assert_eq!(with_stub_quote(&partial, ID, None), whole);
    }

    #[test]
    fn choosing_again_replaces_the_passage() {
        let partial = format!("Lead-in.\n\n![[{ID}]]\n> old passage\n\nAfter.");
        assert_eq!(
            with_stub_quote(&partial, ID, Some("new passage")),
            format!("Lead-in.\n\n![[{ID}]]\n> new passage\n\nAfter.")
        );
    }

    #[test]
    fn an_unmarked_line_right_under_the_quote_is_the_authors() {
        let md = format!("![[{ID}]]\n> old\nmy words");
        assert_eq!(
            with_stub_quote(&md, ID, None),
            format!("![[{ID}]]\nmy words")
        );
    }

    #[test]
    fn the_selection_is_normalized_so_publish_sees_the_same_text() {
        let md = with_stub_quote(
            &format!("![[{ID}]]\n\n"),
            ID,
            Some("  A  line\n\n\n- item one\n"),
        );
        let lines: Vec<&str> = md.split('\n').collect();
        let quote = attached_quote(&lines, 0).0.unwrap();
        assert_eq!(quote, "A line\n\n- item one");
        assert_eq!(crate::selection_from_quote(&quote), "A line\nitem one");
    }

    #[test]
    fn only_the_targets_directive_is_touched_and_never_one_in_code() {
        let md = format!("```\n![[{ID}]]\n```\n\n![[{OTHER}]]\n> theirs\n\n![[{ID}]]\n\nreply");
        assert_eq!(
            with_stub_quote(&md, ID, Some("mine")),
            format!("```\n![[{ID}]]\n```\n\n![[{OTHER}]]\n> theirs\n\n![[{ID}]]\n> mine\n\nreply")
        );
    }

    #[test]
    fn a_body_whose_quote_was_deleted_gains_one_at_the_top() {
        assert_eq!(stub_quote_form("just words", ID), None);
        assert_eq!(
            with_stub_quote("just words", ID, None),
            format!("![[{ID}]]\n\njust words")
        );
    }

    #[test]
    fn a_second_passage_is_added_as_its_own_quote() {
        let md = format!("![[{ID}]]\n> first passage\n\nMy first point.\n\nMy closing point.");
        assert_eq!(passage_count(&md, ID), 1);
        let caret = md.find("My first point.").unwrap() + 3;
        let out = add_stub_quote(&md, ID, "second passage", caret);
        assert_eq!(
            out,
            format!(
                "![[{ID}]]\n> first passage\n\nMy first point.\n\n![[{ID}]]\n> second passage\n\nMy closing point."
            )
        );
        assert_eq!(passage_count(&out, ID), 2);
        // A caret inside a quote never splits it; one at the start appends at the end.
        assert_eq!(
            add_stub_quote(&md, ID, "x", md.find("first passage").unwrap()),
            format!(
                "![[{ID}]]\n> first passage\n\n![[{ID}]]\n> x\n\nMy first point.\n\nMy closing point."
            )
        );
        assert_eq!(
            add_stub_quote(&md, ID, "x", 0),
            format!("{md}\n\n![[{ID}]]\n> x\n\n")
        );
    }

    #[test]
    fn an_empty_passage_changes_nothing() {
        let md = format!("![[{ID}]]\n\n");
        assert_eq!(add_stub_quote(&md, ID, "  \n ", 3), md);
    }

    #[test]
    fn the_hint_names_what_the_draft_does() {
        let whole = stub_body(ID);
        assert!(stub_hint(&whole, ID).starts_with("Quoting the whole post"));
        let one = with_stub_quote(&whole, ID, Some("a"));
        assert!(stub_hint(&one, ID).starts_with("Quoting a passage"));
        let two = add_stub_quote(&one, ID, "b", 0);
        assert!(stub_hint(&two, ID).starts_with("Quoting 2 passages"));
        assert!(stub_hint("words", ID).starts_with("Not quoting"));
    }
}
