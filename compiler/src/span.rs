//! Positions in source text.

/// A half-open byte range `start..end` in a source file.
///
/// Offsets are `u32`, which limits a single file to 4 GiB and keeps tokens small.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub const fn new(start: u32, end: u32) -> Span {
        Span { start, end }
    }

    pub const fn len(self) -> u32 {
        self.end - self.start
    }

    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }

    /// The smallest span covering both `self` and `other`.
    pub const fn to(self, other: Span) -> Span {
        let start = if self.start < other.start {
            self.start
        } else {
            other.start
        };
        let end = if self.end > other.end {
            self.end
        } else {
            other.end
        };
        Span { start, end }
    }
}

/// Maps byte offsets to line and column numbers, for error messages.
pub struct LineIndex {
    /// Byte offset where each line starts. Always begins with 0.
    starts: Vec<u32>,
}

impl LineIndex {
    pub fn new(src: &str) -> LineIndex {
        let mut starts = vec![0];
        for (i, b) in src.bytes().enumerate() {
            if b == b'\n' {
                starts.push((i + 1) as u32);
            }
        }
        LineIndex { starts }
    }

    /// One-based line and column. The column counts characters, not bytes.
    pub fn line_col(&self, src: &str, offset: u32) -> (usize, usize) {
        let line = match self.starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let line_start = self.starts[line] as usize;
        let end = (offset as usize).min(src.len());
        let col = src.get(line_start..end).map_or(0, |s| s.chars().count());
        (line + 1, col + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_both() {
        let a = Span::new(2, 5);
        let b = Span::new(4, 9);
        assert_eq!(a.to(b), Span::new(2, 9));
        assert_eq!(b.to(a), Span::new(2, 9));
    }

    #[test]
    fn line_and_column() {
        let src = "ab\ncd\n\nÃ©x";
        let index = LineIndex::new(src);
        assert_eq!(index.line_col(src, 0), (1, 1));
        assert_eq!(index.line_col(src, 1), (1, 2));
        assert_eq!(index.line_col(src, 3), (2, 1));
        assert_eq!(index.line_col(src, 4), (2, 2));
        assert_eq!(index.line_col(src, 7), (4, 1));
        // 'Ã©' is two bytes but one column.
        assert_eq!(index.line_col(src, 9), (4, 2));
    }

    #[test]
    fn length() {
        assert_eq!(Span::new(3, 7).len(), 4);
        assert!(Span::new(3, 3).is_empty());
    }
}
