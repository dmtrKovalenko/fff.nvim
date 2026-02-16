use std::io;

use crate::searcher::Searcher;

/// A trait that describes errors that can be reported by searchers and
/// implementations of `Sink`.
pub trait SinkError: Sized {
    /// A constructor for converting any value that satisfies the
    /// `std::fmt::Display` trait into an error.
    fn error_message<T: std::fmt::Display>(message: T) -> Self;

    /// A constructor for converting I/O errors that occur while searching into
    /// an error of this type.
    fn error_io(err: io::Error) -> Self {
        Self::error_message(err)
    }
}

impl SinkError for io::Error {
    fn error_message<T: std::fmt::Display>(message: T) -> io::Error {
        io::Error::other(message.to_string())
    }

    fn error_io(err: io::Error) -> io::Error {
        err
    }
}

/// A trait that defines how results from searchers are handled.
///
/// The searcher follows the "push" model: the searcher drives execution and
/// pushes results back to the caller via this trait.
pub trait Sink {
    /// The type of an error that should be reported by a searcher.
    type Error: SinkError;

    /// This method is called whenever a match is found.
    ///
    /// If this returns `true`, then searching continues. If this returns
    /// `false`, then searching is stopped immediately and `finish` is called.
    fn matched(&mut self, _searcher: &Searcher, _mat: &SinkMatch<'_>) -> Result<bool, Self::Error>;

    /// This method is called when a search has begun, before any search is
    /// executed. By default, this does nothing.
    #[inline]
    fn begin(&mut self, _searcher: &Searcher) -> Result<bool, Self::Error> {
        Ok(true)
    }

    /// This method is called when a search has completed. By default, this
    /// does nothing.
    #[inline]
    fn finish(&mut self, _searcher: &Searcher, _: &SinkFinish) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl<S: Sink> Sink for &mut S {
    type Error = S::Error;

    #[inline]
    fn matched(&mut self, searcher: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, S::Error> {
        (**self).matched(searcher, mat)
    }

    #[inline]
    fn begin(&mut self, searcher: &Searcher) -> Result<bool, S::Error> {
        (**self).begin(searcher)
    }

    #[inline]
    fn finish(&mut self, searcher: &Searcher, sink_finish: &SinkFinish) -> Result<(), S::Error> {
        (**self).finish(searcher, sink_finish)
    }
}

/// Summary data reported at the end of a search.
#[derive(Clone, Debug)]
pub struct SinkFinish {
    pub(crate) byte_count: u64,
}

impl SinkFinish {
    /// Return the total number of bytes searched.
    #[inline]
    pub fn byte_count(&self) -> u64 {
        self.byte_count
    }
}

/// A type that describes a match reported by a searcher.
#[derive(Clone, Debug)]
pub struct SinkMatch<'b> {
    pub(crate) bytes: &'b [u8],
    pub(crate) absolute_byte_offset: u64,
    pub(crate) line_number: Option<u64>,
    pub(crate) buffer: &'b [u8],
    pub(crate) bytes_range_in_buffer: std::ops::Range<usize>,
}

impl<'b> SinkMatch<'b> {
    /// Returns the bytes for all matching lines, including the line
    /// terminators, if they exist.
    #[inline]
    pub fn bytes(&self) -> &'b [u8] {
        self.bytes
    }

    /// Returns the absolute byte offset of the start of this match. This
    /// offset is absolute in that it is relative to the very beginning of the
    /// input in a search.
    #[inline]
    pub fn absolute_byte_offset(&self) -> u64 {
        self.absolute_byte_offset
    }

    /// Returns the line number of the first line in this match, if available.
    ///
    /// Line numbers are only available when the search builder is instructed
    /// to compute them.
    #[inline]
    pub fn line_number(&self) -> Option<u64> {
        self.line_number
    }

    /// Exposes as much of the underlying buffer that was searched as possible.
    #[inline]
    pub fn buffer(&self) -> &'b [u8] {
        self.buffer
    }

    /// Returns a range that corresponds to where [`SinkMatch::bytes`] appears
    /// in [`SinkMatch::buffer`].
    #[inline]
    pub fn bytes_range_in_buffer(&self) -> std::ops::Range<usize> {
        self.bytes_range_in_buffer.clone()
    }
}
