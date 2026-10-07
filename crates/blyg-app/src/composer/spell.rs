//! Spellcheck, the pure part: which bytes of a Markdown post are prose worth
//! checking, a per-line cache of results, and keeping underlines in place
//! while the text changes. The checking itself is the system's
//! (`spell_mac.rs`), behind [`SpellEngine`].
//!
//! Incremental by construction: the cache is keyed by a line's masked text,
//! so after an edit only the lines that changed go to the checker.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use super::text::{fence, next_fence_state};

/// The system spell checker, or a fake one in tests. Called off the UI
/// thread for [`SpellEngine::check`].
pub trait SpellEngine: Send + Sync + 'static {
    /// Byte ranges of misspelled words in `text` (one line of masked prose).
    fn check(&self, text: &str) -> Vec<Range<usize>>;
    /// Suggested replacements, best first.
    fn guesses(&self, word: &str) -> Vec<String>;
    /// Add to the user's dictionary (shared with every Mac app).
    fn learn(&self, word: &str);
    /// Stop flagging this word for the rest of the session.
    fn ignore(&self, word: &str);
}

/// A copy of `text` with everything that isn't prose blanked to spaces,
/// byte for byte (newlines kept), so offsets into it are offsets into `text`.
/// Blanked: front matter, fenced code, code spans, link targets and
/// reference definitions, autolinks and HTML tags, `![[id]]`, the
/// `[TK]`/`[=]`/`[/TK]` markers, and bare URLs, domains, emails and `@…`.
pub fn mask(text: &str) -> String {
    let mut b = text.as_bytes().to_vec();
    let mut start = 0;
    // Front matter: a leading `---` line up to the next `---` line.
    if text.starts_with("---\n") || text.starts_with("---\r\n") {
        let mut off = text.find('\n').map_or(text.len(), |i| i + 1);
        while off < text.len() {
            let end = text[off..].find('\n').map_or(text.len(), |i| off + i + 1);
            if text[off..end].trim_end() == "---" {
                blank(&mut b, 0..end);
                start = end;
                break;
            }
            off = end;
        }
    }
    let mut open: Option<(char, usize)> = None;
    let mut off = start;
    while off < text.len() {
        let end = text[off..].find('\n').map_or(text.len(), |i| off + i);
        let line = &text[off..end];
        if let Some(f) = fence(line) {
            open = next_fence_state(open, f, line);
            blank(&mut b, off..end);
        } else if open.is_some() {
            blank(&mut b, off..end);
        } else {
            mask_line(&mut b, off, line);
        }
        off = end + 1;
    }
    // Only whole characters were blanked, so this stays valid UTF-8.
    String::from_utf8(b).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

fn blank(b: &mut [u8], r: Range<usize>) {
    for x in &mut b[r] {
        if *x != b'\n' && *x != b'\r' {
            *x = b' ';
        }
    }
}

/// Mask one line (not in a fence) starting at byte `off` of the text.
fn mask_line(b: &mut [u8], off: usize, line: &str) {
    let lb = line.as_bytes();
    let mut spans: Vec<Range<usize>> = Vec::new();
    // Reference definition: `[label]: https://…`.
    let t = line.trim_start();
    if t.starts_with('[')
        && let Some(close) = t.find("]:")
    {
        let lead = line.len() - t.len();
        spans.push(lead + close + 2..line.len());
    }
    let mut i = 0;
    while i < lb.len() {
        let rest = &line[i..];
        let c = lb[i];
        if c == b'`' {
            let run = rest.bytes().take_while(|x| *x == b'`').count();
            let ticks = &rest[..run];
            let close = rest[run..]
                .match_indices(ticks)
                .find(|(j, _)| {
                    let at = run + j;
                    rest.as_bytes().get(at + run) != Some(&b'`')
                        && rest.as_bytes().get(at.wrapping_sub(1)) != Some(&b'`')
                })
                .map(|(j, _)| run + j + run);
            match close {
                Some(end) => {
                    spans.push(i..i + end);
                    i += end;
                }
                None => {
                    spans.push(i..i + run);
                    i += run;
                }
            }
            continue;
        }
        if rest.starts_with("![[")
            && let Some(end) = rest.find("]]")
        {
            spans.push(i..i + end + 2);
            i += end + 2;
            continue;
        }
        if let Some(m) = ["[TK]", "[/TK]", "[=]"]
            .iter()
            .find(|m| rest.starts_with(**m))
        {
            spans.push(i..i + m.len());
            i += m.len();
            continue;
        }
        if let Some(target) = rest.strip_prefix("](") {
            let end = link_target_end(target).map_or(rest.len(), |e| e + 2);
            spans.push(i + 1..i + end);
            i += end;
            continue;
        }
        if c == b'<'
            && let Some(end) = rest.find('>')
        {
            let inner = &rest[1..end];
            let tagish = inner
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '/' || ch == '!');
            // `<b>`, `</b>`, `<a href="…">`, `<https://…>`.
            if tagish || inner.contains("://") {
                spans.push(i..i + end + 1);
                i += end + 1;
                continue;
            }
        }
        i += rest.chars().next().map_or(1, char::len_utf8);
    }
    for s in &spans {
        blank(b, off + s.start..off + s.end);
    }
    // Bare addresses, domains, emails and `@mentions`, on what's left.
    let masked = std::str::from_utf8(&b[off..off + line.len()])
        .unwrap_or("")
        .to_string();
    let mut tok_start = None;
    for (j, ch) in masked.char_indices().chain([(masked.len(), ' ')]) {
        if ch.is_whitespace() {
            if let Some(s) = tok_start.take() {
                let tok = &masked[s..j];
                if is_address(tok) {
                    blank(b, off + s..off + j);
                }
            }
        } else if tok_start.is_none() {
            tok_start = Some(j);
        }
    }
}

/// Where a link destination after `](` ends (the byte after its `)`), with
/// nested parentheses and `<…>` destinations.
fn link_target_end(s: &str) -> Option<usize> {
    if let Some(inner) = s.strip_prefix('<') {
        let close = inner.find('>')?;
        return Some(1 + close + 1 + inner[close + 1..].find(')')? + 1);
    }
    let mut depth = 0usize;
    for (j, ch) in s.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' if depth == 0 => return Some(j + 1),
            ')' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// A whitespace-delimited token that's an address of some kind, not a word.
fn is_address(tok: &str) -> bool {
    let t = tok.trim_matches(|c: char| "()[]<>\"'“”‘’.,;:!?*_".contains(c));
    if t.is_empty() {
        return false;
    }
    if t.contains("://") || t.starts_with("www.") || t.starts_with("mailto:") || t.contains('@') {
        return true;
    }
    // A domain (`blyg.example.com`, `example.org/path`): dotted, ending in
    // a TLD-ish run of two or more letters.
    let host = t.split('/').next().unwrap_or(t);
    let parts: Vec<&str> = host.split('.').collect();
    parts.len() >= 2
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        && parts
            .last()
            .is_some_and(|tld| tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic()))
}

/// Results per masked line, shared by every editor in the app.
#[derive(Default)]
pub struct Cache {
    lines: HashMap<String, Arc<[Range<usize>]>>,
}

/// What [`Cache::plan`] found: underlines it already knows, and the lines
/// (deduplicated) that still need the checker.
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    pub known: Vec<Range<usize>>,
    pub missing: Vec<String>,
}

/// Lines worth checking: those with a letter in them.
fn lines(masked: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut off = 0;
    masked.split('\n').filter_map(move |l| {
        let start = off;
        off += l.len() + 1;
        l.chars().any(char::is_alphabetic).then_some((start, l))
    })
}

impl Cache {
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// The document's underlines from the cache, plus what's not cached yet.
    pub fn plan(&self, masked: &str) -> Plan {
        let mut plan = Plan::default();
        for (start, l) in lines(masked) {
            match self.lines.get(l) {
                Some(rs) => plan
                    .known
                    .extend(rs.iter().map(|r| start + r.start..start + r.end)),
                None => {
                    if !plan.missing.iter().any(|m| m == l) {
                        plan.missing.push(l.to_string());
                    }
                }
            }
        }
        plan
    }

    pub fn insert(&mut self, line: String, ranges: Vec<Range<usize>>) {
        // A long session can't grow it without bound.
        if self.lines.len() > 20_000 {
            self.lines.clear();
        }
        self.lines.insert(line, ranges.into());
    }

    /// Forget every line containing `word` (after Learn or Ignore).
    pub fn forget_word(&mut self, word: &str) {
        self.lines.retain(|l, _| !l.contains(word));
    }
}

/// Keep underlines on their words across an edit of `edit` (old byte
/// range) replaced by `inserted` bytes. An underline the edit touches
/// (typing on at a word's end included) is dropped until the next check.
pub fn shift(ranges: &mut Vec<Range<usize>>, edit: &Range<usize>, inserted: usize) {
    ranges.retain_mut(|r| {
        if r.end < edit.start {
            return true;
        }
        if r.start > edit.end {
            let new_start = r.start - edit.len() + inserted;
            *r = new_start..new_start + (r.end - r.start);
            return true;
        }
        false
    });
}

/// The underline under (or touching) byte `offset`.
pub fn at(ranges: &[Range<usize>], offset: usize) -> Option<Range<usize>> {
    ranges
        .iter()
        .find(|r| r.start <= offset && offset <= r.end)
        .cloned()
}

/// Drop the word being typed right now (ending at the caret): it isn't
/// finished, so it isn't wrong yet.
pub fn settle(mut ranges: Vec<Range<usize>>, cursor: usize) -> Vec<Range<usize>> {
    ranges.retain(|r| r.end != cursor);
    ranges
}

/// A dictionary engine for tests: flags the words in `bad`, and remembers
/// what's learned or ignored. Counts the lines it's asked to check.
#[cfg(test)]
pub mod fake {
    use std::collections::{HashMap, HashSet};
    use std::ops::Range;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    pub struct FakeSpell {
        bad: HashMap<String, Vec<String>>,
        ok: Mutex<HashSet<String>>,
        pub checked_lines: AtomicUsize,
    }

    impl FakeSpell {
        /// `bad`: `(word, suggestions)`.
        pub fn new(bad: &[(&str, &[&str])]) -> Self {
            Self {
                bad: bad
                    .iter()
                    .map(|(w, g)| (w.to_string(), g.iter().map(|s| s.to_string()).collect()))
                    .collect(),
                ..Default::default()
            }
        }

        pub fn checked(&self) -> usize {
            self.checked_lines.load(Ordering::SeqCst)
        }
    }

    impl super::SpellEngine for FakeSpell {
        fn check(&self, text: &str) -> Vec<Range<usize>> {
            self.checked_lines.fetch_add(1, Ordering::SeqCst);
            let ok = self.ok.lock().unwrap();
            let mut out = Vec::new();
            let mut start = None;
            for (i, c) in text.char_indices().chain([(text.len(), ' ')]) {
                if c.is_alphabetic() || c == '\'' {
                    start.get_or_insert(i);
                } else if let Some(s) = start.take() {
                    let w = &text[s..i];
                    if self.bad.contains_key(w) && !ok.contains(w) {
                        out.push(s..i);
                    }
                }
            }
            out
        }
        fn guesses(&self, word: &str) -> Vec<String> {
            self.bad.get(word).cloned().unwrap_or_default()
        }
        fn learn(&self, word: &str) {
            self.ok.lock().unwrap().insert(word.to_string());
        }
        fn ignore(&self, word: &str) {
            self.ok.lock().unwrap().insert(word.to_string());
        }
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    /// The words `mask` leaves, in order.
    fn prose(text: &str) -> Vec<String> {
        let m = mask(text);
        assert_eq!(m.len(), text.len(), "byte for byte");
        m.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn mask_keeps_prose_and_offsets() {
        let t = "Teh quick fox.\nSecond line";
        assert_eq!(mask(t), t);
        assert_eq!(prose("Café ünïcode"), ["Café", "ünïcode"]);
    }

    #[test]
    fn mask_skips_code() {
        assert_eq!(prose("say `fooo bar` now"), ["say", "now"]);
        assert_eq!(prose("say ``a ` b`` now"), ["say", "now"]);
        assert_eq!(
            prose("Before\n```rust\nlet zzz = 1;\n```\nAfter"),
            ["Before", "After"]
        );
        assert_eq!(
            prose("Open\n~~~\nqqq\n"),
            ["Open"],
            "unclosed fence runs to the end"
        );
    }

    #[test]
    fn mask_skips_links_and_addresses() {
        assert_eq!(
            prose("See [the post](https://blyg.example.com/f/01J(x)) today"),
            ["See", "[the", "post]", "today"]
        );
        assert_eq!(prose("[a](<https://x.example.org/a b>) ok"), ["[a]", "ok"]);
        assert_eq!(prose("go https://blyg.example.com/f/1 now"), ["go", "now"]);
        assert_eq!(
            prose("at blyg.example.org, or www.example.net."),
            ["at", "or"]
        );
        assert_eq!(prose("mail ada@example.org or @ada"), ["mail", "or"]);
        assert_eq!(prose("e.g. this"), ["e.g.", "this"], "not a domain");
        assert_eq!(
            prose("<https://x.example.org> and <b>bold</b>"),
            ["and", "bold"]
        );
        assert_eq!(prose("[ref]: https://x.example.org/y \"T\""), ["[ref]:"]);
    }

    #[test]
    fn mask_skips_blyg_markup() {
        assert_eq!(prose("Quote:\n![[01J9ABCDEF]]\nend"), ["Quote:", "end"]);
        assert_eq!(
            prose("[TK]write a line[=]A line.[/TK]"),
            ["write", "a", "line", "A", "line."]
        );
        assert_eq!(
            prose("---\ntitle: Zzyzx\n---\nBody"),
            ["Body"],
            "front matter"
        );
        assert_eq!(
            prose("--- not front matter"),
            ["---", "not", "front", "matter"]
        );
    }

    #[test]
    fn plan_asks_only_for_uncached_lines() {
        let mut c = Cache::default();
        let text = "Teh one\n\nall good\nTeh one";
        let p = c.plan(text);
        assert!(p.known.is_empty());
        assert_eq!(
            p.missing,
            ["Teh one", "all good"],
            "deduplicated, blank skipped"
        );
        c.insert("Teh one".into(), vec![0..3]);
        c.insert("all good".into(), vec![]);
        let p = c.plan(text);
        assert!(p.missing.is_empty());
        assert_eq!(p.known, [0..3, 18..21]);
        // Edit one line: only it is missing.
        let p = c.plan("Teh one\n\nall goood\nTeh one");
        assert_eq!(p.missing, ["all goood"]);
        c.forget_word("Teh");
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn shift_follows_edits_and_drops_touched_words() {
        let mut r = vec![0..3, 10..14, 20..24];
        shift(&mut r, &(5..5), 2); // typed two bytes between words
        assert_eq!(r, [0..3, 12..16, 22..26]);
        shift(&mut r, &(16..16), 1); // typing on at the end of a word
        assert_eq!(r, [0..3, 23..27]);
        shift(&mut r, &(1..2), 0); // deleting inside a word
        assert_eq!(r, [22..26]);
        shift(&mut r, &(0..10), 0); // a deletion before
        assert_eq!(r, [12..16]);
    }

    #[test]
    fn at_and_settle() {
        let r = vec![0..3, 10..14];
        assert_eq!(at(&r, 3), Some(0..3));
        assert_eq!(at(&r, 5), None);
        assert_eq!(settle(r, 14), [0..3]);
    }
}
