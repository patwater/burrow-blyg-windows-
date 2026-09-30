//! Text with a per-line map back to the author's source, carried through the
//! preview pipeline's string rewrites (TK stripping, sentinel annotation,
//! transclusion line splitting) so every rendered top-level block can report
//! the source line it came from.

use crate::markdown::{line_index, line_starts};

#[derive(Debug, Clone)]
pub(crate) struct Mapped {
    pub text: String,
    /// `lines[i]` is the source line (0-based) that line `i` of `text` came from.
    pub lines: Vec<usize>,
    starts: Vec<usize>,
}

impl Mapped {
    pub fn identity(text: &str) -> Self {
        let starts = line_starts(text);
        Mapped {
            text: text.to_string(),
            lines: (0..starts.len()).collect(),
            starts,
        }
    }

    /// Source line of the byte at `offset` in `text`.
    pub fn line_at(&self, offset: usize) -> usize {
        self.lines[line_index(&self.starts, offset)]
    }
}

pub(crate) struct MapBuilder<'a> {
    prev: &'a Mapped,
    out: String,
    lines: Vec<usize>,
}

impl<'a> MapBuilder<'a> {
    pub fn new(prev: &'a Mapped) -> Self {
        MapBuilder {
            prev,
            out: String::with_capacity(prev.text.len() + 16),
            lines: Vec::new(),
        }
    }

    /// Append `prev.text[from..to]`, mapping its lines through `prev`.
    pub fn copy(&mut self, from: usize, to: usize) {
        if from == to {
            return;
        }
        let piece = &self.prev.text[from..to];
        if self.lines.is_empty() {
            self.lines.push(self.prev.line_at(from));
        }
        for (r, _) in piece.match_indices('\n') {
            self.lines.push(self.prev.line_at(from + r + 1));
        }
        self.out.push_str(piece);
    }

    /// Append text that is not in `prev`; its lines all map to `line`.
    pub fn insert(&mut self, piece: &str, line: usize) {
        if piece.is_empty() {
            return;
        }
        if self.lines.is_empty() {
            self.lines.push(line);
        }
        for _ in piece.matches('\n') {
            self.lines.push(line);
        }
        self.out.push_str(piece);
    }

    /// Length in bytes of the text built so far.
    pub fn len(&self) -> usize {
        self.out.len()
    }

    pub fn finish(mut self) -> Mapped {
        if self.lines.is_empty() {
            self.lines
                .push(self.prev.lines.first().copied().unwrap_or(0));
        }
        let starts = line_starts(&self.out);
        debug_assert_eq!(starts.len(), self.lines.len());
        Mapped {
            text: self.out,
            lines: self.lines,
            starts,
        }
    }
}
