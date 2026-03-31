//! Matcher trait inspired by ripgrep's `Matcher` just simpler

/// A byte range representing a match.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Match {
    start: usize,
    end: usize,
}

impl Match {
    /// Create a new match from start/end byte offsets.
    #[inline]
    pub fn new(start: usize, end: usize) -> Match {
        debug_assert!(start <= end);
        Match { start, end }
    }

    /// Create a zero-width match at `offset`.
    #[inline]
    pub fn zero(offset: usize) -> Match {
        Match {
            start: offset,
            end: offset,
        }
    }

    /// Start byte offset.
    #[inline]
    pub fn start(&self) -> usize {
        self.start
    }

    /// End byte offset (exclusive).
    #[inline]
    pub fn end(&self) -> usize {
        self.end
    }

    /// Return a copy with a different end offset.
    #[inline]
    pub fn with_end(&self, end: usize) -> Match {
        debug_assert!(self.start <= end);
        Match { end, ..*self }
    }

    /// Shift both offsets forward by `amount`.
    #[inline]
    pub fn offset(&self, amount: usize) -> Match {
        Match {
            start: self.start + amount,
            end: self.end + amount,
        }
    }

    /// Byte length of the match.
    #[inline]
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    /// True if this is a zero-width match.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl std::ops::Index<Match> for [u8] {
    type Output = [u8];

    #[inline]
    fn index(&self, index: Match) -> &[u8] {
        &self[index.start..index.end]
    }
}

impl std::ops::IndexMut<Match> for [u8] {
    #[inline]
    fn index_mut(&mut self, index: Match) -> &mut [u8] {
        &mut self[index.start..index.end]
    }
}

impl std::ops::Index<Match> for str {
    type Output = str;

    #[inline]
    fn index(&self, index: Match) -> &str {
        &self[index.start..index.end]
    }
}

/// A line terminator (always a single byte for fff — no CRLF support needed).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct LineTerminator(u8);

impl LineTerminator {
    /// Create a line terminator from a single byte.
    #[inline]
    pub fn byte(byte: u8) -> LineTerminator {
        LineTerminator(byte)
    }

    /// Return the terminator byte.
    #[inline]
    pub fn as_byte(&self) -> u8 {
        self.0
    }

    /// Return the terminator as a single-element byte slice.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        std::slice::from_ref(&self.0)
    }
}

impl Default for LineTerminator {
    #[inline]
    fn default() -> LineTerminator {
        LineTerminator(b'\n')
    }
}

/// An error type for matchers that never produce errors.
#[derive(Debug, Eq, PartialEq)]
pub struct NoError(());

impl std::error::Error for NoError {}

impl std::fmt::Display for NoError {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        unreachable!("NoError should never be instantiated")
    }
}

/// A matcher finds byte-level matches in a haystack.
pub trait Matcher {
    /// The error type (use [`NoError`] for infallible matchers).
    type Error: std::fmt::Display;

    /// Find the first match at or after `at` in `haystack`.
    fn find_at(&self, haystack: &[u8], at: usize) -> Result<Option<Match>, Self::Error>;

    /// Find the first match in `haystack`.
    #[inline]
    fn find(&self, haystack: &[u8]) -> Result<Option<Match>, Self::Error> {
        self.find_at(haystack, 0)
    }

    /// The line terminator this matcher guarantees will never appear in a match.
    /// Return `None` if the matcher can match across lines.
    #[inline]
    fn line_terminator(&self) -> Option<LineTerminator> {
        None
    }
}

impl<M: Matcher> Matcher for &M {
    type Error = M::Error;

    #[inline]
    fn find_at(&self, haystack: &[u8], at: usize) -> Result<Option<Match>, Self::Error> {
        (*self).find_at(haystack, at)
    }

    #[inline]
    fn find(&self, haystack: &[u8]) -> Result<Option<Match>, Self::Error> {
        (*self).find(haystack)
    }

    #[inline]
    fn line_terminator(&self) -> Option<LineTerminator> {
        (*self).line_terminator()
    }
}
