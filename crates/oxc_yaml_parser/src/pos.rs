/// Span represents a range of a piece of source code.
/// It counts by byte offset, so it's 0-based.
///
/// Adapted from saphyr's `Span` (see the note in `scanner.rs`),
/// with line/column markers replaced by byte offsets.
///
/// Offsets are `u32` (matching oxc convention); sources larger than 4 GiB are
/// rejected by the parser up front.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct Span {
    /// Start offset. (Inclusive)
    pub start: u32,
    /// End offset. (Exclusive)
    pub end: u32,
}

impl Span {
    pub fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    /// A zero-width span at the given offset. Covers no characters, but its
    /// position may still be meaningful (e.g. a marker between two tokens).
    pub fn empty(at: u32) -> Self {
        Self { start: at, end: at }
    }

    /// The source text this span covers.
    pub fn slice(self, source: &str) -> &str {
        &source[self.start as usize..self.end as usize]
    }

    /// Whether the span covers no characters. An empty span's position may
    /// still be meaningful (e.g. a synthesized token).
    pub fn is_empty(self) -> bool {
        self.start == self.end
    }
}
