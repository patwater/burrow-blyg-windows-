//! The macOS system spell checker (`NSSpellChecker`): the user's languages,
//! their learned words, no bundled dictionary.

use std::ops::Range;
use std::sync::Mutex;

use objc2::rc::autoreleasepool;
use objc2_app_kit::NSSpellChecker;
use objc2_foundation::{NSRange, NSString, NSTextCheckingType};

use super::spell::SpellEngine;

pub struct MacSpell {
    /// One spell "document" for the app: Ignore lasts for the session.
    tag: isize,
    /// One check at a time (checks run on GPUI's background threads).
    lock: Mutex<()>,
}

impl MacSpell {
    /// Call on the main thread (it creates the shared checker there).
    pub fn new() -> Self {
        let _ = NSSpellChecker::sharedSpellChecker();
        Self {
            tag: NSSpellChecker::uniqueSpellDocumentTag(),
            lock: Mutex::new(()),
        }
    }
}

/// Byte offset for every UTF-16 offset of `s` (and one past the end).
fn utf16_to_byte(s: &str) -> Vec<usize> {
    let mut map = Vec::with_capacity(s.len() + 1);
    for (b, c) in s.char_indices() {
        for _ in 0..c.len_utf16() {
            map.push(b);
        }
    }
    map.push(s.len());
    map
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

impl SpellEngine for MacSpell {
    fn check(&self, text: &str) -> Vec<Range<usize>> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        autoreleasepool(|_| {
            let ns = NSString::from_str(text);
            let checker = NSSpellChecker::sharedSpellChecker();
            // SAFETY: no options dictionary, no orthography out-param, and a
            // null word count, all of which the API allows.
            let results = unsafe {
                checker
                    .checkString_range_types_options_inSpellDocumentWithTag_orthography_wordCount(
                        &ns,
                        NSRange::new(0, utf16_len(text)),
                        NSTextCheckingType::Spelling.0,
                        None,
                        self.tag,
                        None,
                        std::ptr::null_mut(),
                    )
            };
            let map = utf16_to_byte(text);
            results
                .iter()
                .filter_map(|r| {
                    let range = r.range();
                    let start = *map.get(range.location)?;
                    let end = *map.get(range.location + range.length)?;
                    (start < end).then_some(start..end)
                })
                .collect()
        })
    }

    fn guesses(&self, word: &str) -> Vec<String> {
        autoreleasepool(|_| {
            let ns = NSString::from_str(word);
            let checker = NSSpellChecker::sharedSpellChecker();
            checker
                .guessesForWordRange_inString_language_inSpellDocumentWithTag(
                    NSRange::new(0, utf16_len(word)),
                    &ns,
                    None,
                    self.tag,
                )
                .map(|a| a.iter().map(|s| s.to_string()).collect())
                .unwrap_or_default()
        })
    }

    fn learn(&self, word: &str) {
        autoreleasepool(|_| {
            NSSpellChecker::sharedSpellChecker().learnWord(&NSString::from_str(word));
        })
    }

    fn ignore(&self, word: &str) {
        autoreleasepool(|_| {
            NSSpellChecker::sharedSpellChecker()
                .ignoreWord_inSpellDocumentWithTag(&NSString::from_str(word), self.tag);
        })
    }
}

#[cfg(test)]
mod tests {
    use super::utf16_to_byte;

    #[test]
    fn utf16_offsets_map_to_bytes() {
        let m = utf16_to_byte("aé😀b");
        // a=0, é=1 (2 bytes), 😀=3 (4 bytes, two UTF-16 units), b=7
        assert_eq!(m, [0, 1, 3, 3, 7, 8]);
    }
}

/// Against the real system checker (manual: `cargo test -p blyg-app --release
/// system_checker -- --ignored --nocapture`). Prints how long a check takes.
#[cfg(test)]
mod system {
    use super::super::spell::{SpellEngine, mask};
    use super::MacSpell;

    #[test]
    #[ignore]
    fn system_checker_flags_typos_and_is_quick() {
        let s = MacSpell::new();
        let line = "The tide tabels are a promiss the sea never signd.";
        let found: Vec<&str> = s.check(line).into_iter().map(|r| &line[r]).collect();
        println!(
            "flagged: {found:?}; guesses for tabels: {:?}",
            s.guesses("tabels")
        );
        assert!(found.contains(&"tabels"), "{found:?}");
        let doc = format!("{line}\n\n").repeat(560);
        let masked = mask(&doc);
        let t0 = std::time::Instant::now();
        let mut n = 0;
        for l in masked.lines().filter(|l| !l.trim().is_empty()).take(200) {
            n += s.check(l).len();
        }
        let per_line = t0.elapsed().as_micros() as f64 / 200.0;
        println!("system_checker: {per_line:.0} µs per line ({n} flags in 200 lines)");
    }
}
