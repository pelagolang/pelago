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
    fn length() {
        assert_eq!(Span::new(3, 7).len(), 4);
        assert!(Span::new(3, 3).is_empty());
    }
}
