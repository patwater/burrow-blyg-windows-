//! Small Markdown facts the composer needs: are we inside code?

/// Whether text ending here leaves a ``` or ~~~ fence open.
pub fn in_code_fence(before: &str) -> bool {
    let mut open: Option<(char, usize)> = None;
    for line in before.lines() {
        if let Some(f) = fence(line) {
            open = next_fence_state(open, f, line);
        }
    }
    open.is_some()
}

/// A line that opens or closes a fence: its character and run length.
pub fn fence(line: &str) -> Option<(char, usize)> {
    let t = line.trim_start_matches(' ');
    let c = t.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let run = t.chars().take_while(|x| *x == c).count();
    (run >= 3).then_some((c, run))
}

/// The fence state after a fence line (`open` is the fence we're inside).
pub fn next_fence_state(
    open: Option<(char, usize)>,
    (c, run): (char, usize),
    line: &str,
) -> Option<(char, usize)> {
    match open {
        None => Some((c, run)),
        Some((oc, orun))
            if oc == c && run >= orun && line.trim_start_matches(' ')[run..].trim().is_empty() =>
        {
            None
        }
        Some(o) => Some(o),
    }
}

/// Whether text on one line, ending here, is inside a `code span` (an
/// unclosed run of backticks).
pub fn in_inline_code(line_before: &str) -> bool {
    let b = line_before.as_bytes();
    let mut i = 0;
    let mut open: Option<usize> = None;
    while i < b.len() {
        if b[i] == b'`' {
            let start = i;
            while i < b.len() && b[i] == b'`' {
                i += 1;
            }
            let run = i - start;
            match open {
                None => open = Some(run),
                Some(r) if r == run => open = None,
                Some(_) => {}
            }
        } else {
            i += 1;
        }
    }
    open.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fences_open_and_close() {
        assert!(!in_code_fence(""));
        assert!(in_code_fence("```\n"));
        assert!(in_code_fence("~~~md\ncode\n"));
        assert!(!in_code_fence("```\ncode\n```\n"));
        assert!(in_code_fence("````\n```\n"), "a shorter run doesn't close");
        assert!(
            in_code_fence("```\n~~~\n"),
            "another character doesn't close"
        );
    }

    #[test]
    fn code_spans() {
        assert!(!in_inline_code("plain"));
        assert!(in_inline_code("see `x"));
        assert!(!in_inline_code("see `x` and"));
        assert!(in_inline_code("``a ` b"), "a single tick inside a double");
    }
}
