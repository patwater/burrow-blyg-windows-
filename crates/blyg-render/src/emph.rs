//! Emphasis-like pairs (`*em*`, `__strong__`, `~~strike~~`).
//!
//! A copy of markdown-it.rs 0.6.1 `generics/inline/emph_pair.rs` (MIT,
//! Copyright (c) 2022 Alex Kocharin) with one change: delimiter runs are
//! classified as markdown-it 14 does (CommonMark 0.31), where Unicode
//! *symbols* count as punctuation (`isPunctChar = P || S`) and whitespace is
//! markdown-it's `isWhiteSpace`. markdown-it.rs only treats `P` as
//! punctuation, so `*€*x` differed (CommonMark example 354).
#![allow(clippy::all)]

use std::cmp::min;

use markdown_it::common::sourcemap::SourcePos;
use markdown_it::parser::core::CoreRule;
use markdown_it::parser::extset::{MarkdownItExt, NodeExt};
use markdown_it::parser::inline::builtin::InlineParserRule;
use markdown_it::parser::inline::{InlineRule, InlineState, Text};
use markdown_it::{MarkdownIt, Node, NodeValue};

#[derive(Debug, Default)]
struct PairConfig<const MARKER: char> {
    inserted: bool,
    fns: [Option<fn() -> Node>; 3],
}
impl<const MARKER: char> MarkdownItExt for PairConfig<MARKER> {}

#[derive(Debug, Default)]
struct OpenersBottom<const MARKER: char>([usize; 6]);
impl<const MARKER: char> NodeExt for OpenersBottom<MARKER> {}

#[derive(Debug, Clone)]
#[doc(hidden)]
pub struct EmphMarker {
    // Starting marker
    pub marker: char,

    // Total length of these series of delimiters.
    pub length: usize,

    // Remaining length that's not already matched to other delimiters.
    pub remaining: usize,

    // Boolean flags that determine if this delimiter could open or close
    // an emphasis.
    pub open: bool,
    pub close: bool,
}

// this node is supposed to be replaced by actual emph or text node
impl NodeValue for EmphMarker {}

pub fn add_with<const MARKER: char, const LENGTH: u8, const CAN_SPLIT_WORD: bool>(
    md: &mut MarkdownIt,
    f: fn() -> Node,
) {
    let pair_config = md.ext.get_or_insert_default::<PairConfig<MARKER>>();
    pair_config.fns[LENGTH as usize - 1] = Some(f);

    if !pair_config.inserted {
        pair_config.inserted = true;
        md.inline
            .add_rule::<EmphPairScanner<MARKER, CAN_SPLIT_WORD>>();
    }

    if !md.has_rule::<FragmentsJoin>() {
        md.add_rule::<FragmentsJoin>()
            .before_all()
            .after::<InlineParserRule>();
    }
}

#[doc(hidden)]
pub struct EmphPairScanner<const MARKER: char, const CAN_SPLIT_WORD: bool>;
impl<const MARKER: char, const CAN_SPLIT_WORD: bool> InlineRule
    for EmphPairScanner<MARKER, CAN_SPLIT_WORD>
{
    const MARKER: char = MARKER;

    // this rule works on a closing marker, so for technical reasons any rules trying to skip it
    // should see just plain text
    fn check(_: &mut InlineState) -> Option<usize> {
        None
    }

    fn run(state: &mut InlineState) -> Option<(Node, usize)> {
        let mut chars = state.src[state.pos..state.pos_max].chars();
        if chars.next().unwrap() != MARKER {
            return None;
        }

        let scanned = scan_delims(state, state.pos, CAN_SPLIT_WORD);
        let mut node = Node::new(EmphMarker {
            marker: MARKER,
            length: scanned.length,
            remaining: scanned.length,
            open: scanned.can_open,
            close: scanned.can_close,
        });
        node.srcmap = state.get_map(state.pos, state.pos + scanned.length);
        node = scan_and_match_delimiters::<MARKER>(state, node);
        let map = node.srcmap.unwrap().get_byte_offsets();
        // backtrack to keep correct source maps
        state.pos += scanned.length;
        let token_len = map.1 - map.0;
        state.pos -= token_len;
        Some((node, token_len))
    }
}

/// Assuming last token is a closing delimiter we just inserted,
/// try to find opener(s). If any are found, move stuff to nested emph node.
fn scan_and_match_delimiters<const MARKER: char>(
    state: &mut InlineState,
    mut closer_token: Node,
) -> Node {
    if state.node.children.is_empty() {
        return closer_token;
    } // must have at least opener and closer

    let mut closer = closer_token.cast_mut::<EmphMarker>().unwrap().clone();
    if !closer.close {
        return closer_token;
    }

    // Previously calculated lower bounds (previous fails)
    // for each marker, each delimiter length modulo 3,
    // and for whether this closer can be an opener;
    // https://github.com/commonmark/cmark/commit/34250e12ccebdc6372b8b49c44fab57c72443460
    let openers_for_marker = state
        .node
        .ext
        .get_or_insert_default::<OpenersBottom<MARKER>>();
    let openers_parameter = (closer.open as usize) * 3 + closer.length % 3;

    let min_opener_idx = openers_for_marker.0[openers_parameter];

    let mut idx = state.node.children.len() - 1;
    let mut new_min_opener_idx = idx;
    while idx > min_opener_idx {
        idx -= 1;

        let Some(opener) = state.node.children[idx].cast::<EmphMarker>() else {
            continue;
        };

        let mut opener = opener.clone();
        if opener.open && opener.marker == closer.marker && !is_odd_match(&opener, &closer) {
            while closer.remaining > 0 && opener.remaining > 0 {
                let max_marker_len = min(3, min(opener.remaining, closer.remaining));
                let mut matched_rule = None;
                let fns = &state.md.ext.get::<PairConfig<MARKER>>().unwrap().fns;
                for marker_len in (1..=max_marker_len).rev() {
                    if let Some(f) = fns[marker_len - 1] {
                        matched_rule = Some((marker_len, f));
                        break;
                    }
                }

                // If matched_fn isn't found, it can only mean that function is defined for larger marker
                // than we have (e.g. function defined for **, we have *).
                // Treat this as "marker not found".
                if matched_rule.is_none() {
                    break;
                }

                let (marker_len, marker_fn) = matched_rule.unwrap();

                closer.remaining -= marker_len;
                opener.remaining -= marker_len;

                let mut new_token = marker_fn();
                new_token.children = state.node.children.split_off(idx + 1);

                // cut marker_len chars from start, i.e. "12345" -> "345"
                let mut end_map_pos = 0;
                if let Some(map) = closer_token.srcmap {
                    let (start, end) = map.get_byte_offsets();
                    closer_token.srcmap = Some(SourcePos::new(start + marker_len, end));
                    end_map_pos = start + marker_len;
                }

                // cut marker_len chars from end, i.e. "12345" -> "123"
                let mut start_map_pos = 0;
                let opener_token = state.node.children.last_mut().unwrap();
                if let Some(map) = opener_token.srcmap {
                    let (start, end) = map.get_byte_offsets();
                    opener_token.srcmap = Some(SourcePos::new(start, end - marker_len));
                    start_map_pos = end - marker_len;
                }

                new_token.srcmap = state.get_map(start_map_pos, end_map_pos);

                // remove empty node as a small optimization so we can do less work later
                if opener.remaining == 0 {
                    state.node.children.pop();
                }

                new_min_opener_idx = 0;
                state.node.children.push(new_token);
            }
        }

        if opener.remaining > 0 {
            state.node.children[idx].replace(opener);
        } // otherwise node was already deleted
    }

    if new_min_opener_idx != 0 {
        // If match for this delimiter run failed, we want to set lower bound for
        // future lookups. This is required to make sure algorithm has linear
        // complexity.
        //
        // See details here:
        // https://github.com/commonmark/cmark/issues/178#issuecomment-270417442
        //
        let openers_for_marker = state
            .node
            .ext
            .get_or_insert_default::<OpenersBottom<MARKER>>();
        openers_for_marker.0[openers_parameter] = new_min_opener_idx;
    }

    // remove empty node as a small optimization so we can do less work later
    if closer.remaining > 0 {
        closer_token.replace(closer);
        closer_token
    } else {
        state.node.children.pop().unwrap()
    }
}

fn is_odd_match(opener: &EmphMarker, closer: &EmphMarker) -> bool {
    // from spec:
    //
    // If one of the delimiters can both open and close emphasis, then the
    // sum of the lengths of the delimiter runs containing the opening and
    // closing delimiters must not be a multiple of 3 unless both lengths
    // are multiples of 3.
    //
    #[allow(clippy::collapsible_if)]
    if opener.close || closer.open {
        if (opener.length + closer.length) % 3 == 0 {
            if opener.length % 3 != 0 || closer.length % 3 != 0 {
                return true;
            }
        }
    }

    false
}

#[doc(hidden)]
pub struct FragmentsJoin;
impl CoreRule for FragmentsJoin {
    fn run(node: &mut Node, _: &MarkdownIt) {
        node.walk_mut(|node, _| fragments_join(node));
    }
}

/// Clean up tokens after emphasis and strikethrough postprocessing:
/// merge adjacent text nodes into one and re-calculate all token levels
///
/// This is necessary because initially emphasis delimiter markers (*, _, ~)
/// are treated as their own separate text tokens. Then emphasis rule either
/// leaves them as text (needed to merge with adjacent text) or turns them
/// into opening/closing tags (which messes up levels inside).
///
fn fragments_join(node: &mut Node) {
    // replace all emph markers with text tokens
    for token in node.children.iter_mut() {
        if let Some(data) = token.cast::<EmphMarker>() {
            let content = data.marker.to_string().repeat(data.remaining);
            token.replace(Text { content });
        }
    }

    // collapse adjacent text tokens
    for idx in 1..node.children.len() {
        let (tokens1, tokens2) = node.children.split_at_mut(idx);

        let token1 = tokens1.last_mut().unwrap();
        let Some(t1_data) = token1.cast_mut::<Text>() else {
            continue;
        };

        let token2 = tokens2.first_mut().unwrap();
        let Some(t2_data) = token2.cast_mut::<Text>() else {
            continue;
        };

        // concat contents
        let t2_content = std::mem::take(&mut t2_data.content);
        t1_data.content += &t2_content;

        // adjust source maps
        if let Some(map1) = token1.srcmap {
            if let Some(map2) = token2.srcmap {
                token1.srcmap = Some(SourcePos::new(
                    map1.get_byte_offsets().0,
                    map2.get_byte_offsets().1,
                ));
            }
        }

        node.children.swap(idx - 1, idx);
    }

    // remove all empty tokens
    node.children.retain(|token| {
        if let Some(data) = token.cast::<Text>() {
            !data.content.is_empty()
        } else {
            true
        }
    });
}

/// markdown-it `isWhiteSpace`.
fn is_md_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{2000}'
            ..='\u{200A}'
                | '\t'
                | '\n'
                | '\u{0B}'
                | '\u{0C}'
                | '\r'
                | ' '
                | '\u{A0}'
                | '\u{1680}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
    )
}

/// markdown-it `isMdAsciiPunct(c) || isPunctChar(c)`: ASCII punctuation, or
/// Unicode general category P or S.
fn is_md_punct(c: char) -> bool {
    use unicode_general_category::GeneralCategory::*;
    use unicode_general_category::get_general_category;
    if c.is_ascii() {
        return c.is_ascii_punctuation();
    }
    matches!(
        get_general_category(c),
        ConnectorPunctuation
            | DashPunctuation
            | OpenPunctuation
            | ClosePunctuation
            | InitialPunctuation
            | FinalPunctuation
            | OtherPunctuation
            | MathSymbol
            | CurrencySymbol
            | ModifierSymbol
            | OtherSymbol
    )
}

struct DelimiterRun {
    can_open: bool,
    can_close: bool,
    length: usize,
}

/// markdown-it 14 `StateInline#scanDelims`.
fn scan_delims(state: &InlineState, start: usize, can_split_word: bool) -> DelimiterRun {
    let last_char = if start > 0 {
        state.src[..start].chars().next_back().unwrap()
    } else {
        ' '
    };
    let rest = &state.src[start..state.pos_max];
    let marker = rest.chars().next().unwrap();
    let count = rest.chars().take_while(|&c| c == marker).count();
    let next_char = rest.chars().nth(count).unwrap_or(' ');

    let is_last_punct = is_md_punct(last_char);
    let is_next_punct = is_md_punct(next_char);
    let is_last_ws = is_md_whitespace(last_char);
    let is_next_ws = is_md_whitespace(next_char);

    let left_flanking = !is_next_ws && (!is_next_punct || is_last_ws || is_last_punct);
    let right_flanking = !is_last_ws && (!is_last_punct || is_next_ws || is_next_punct);

    let (can_open, can_close) = if !can_split_word {
        (
            left_flanking && (!right_flanking || is_last_punct),
            right_flanking && (!left_flanking || is_next_punct),
        )
    } else {
        (left_flanking, right_flanking)
    };
    DelimiterRun {
        can_open,
        can_close,
        length: count,
    }
}
