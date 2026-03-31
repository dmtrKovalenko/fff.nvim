/*!
Simplified grep-searcher for fff.nvim.

Provides line-oriented search over byte slices with optional multi-line support.
Only `search_slice` is supported -- no file/reader/mmap search.
*/

#![deny(missing_docs)]

pub use crate::{
    matcher::{LineTerminator, Match, Matcher, NoError},
    searcher::{Searcher, SearcherBuilder},
    sink::{Sink, SinkError, SinkFinish, SinkMatch},
};

pub mod lines;
pub mod matcher;
mod searcher;
mod sink;
