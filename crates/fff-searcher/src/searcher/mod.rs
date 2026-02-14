use grep_matcher::{LineTerminator, Match, Matcher};

use crate::{
    searcher::glue::{MultiLine, SliceByLine},
    sink::{Sink, SinkError},
};

mod core;
mod glue;

/// We use this type alias since we want the ergonomics of a matcher's `Match`
/// type, but in practice, we use it for arbitrary ranges, so give it a more
/// accurate name. This is only used in the searcher's internals.
type Range = Match;

/// An error that can occur when building a searcher.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub(crate) enum ConfigError {
    /// Occurs when a matcher reports a line terminator that is different than
    /// the one configured in the searcher.
    MismatchedLineTerminators {
        /// The matcher's line terminator.
        matcher: LineTerminator,
        /// The searcher's line terminator.
        searcher: LineTerminator,
    },
}

impl std::error::Error for ConfigError {}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            ConfigError::MismatchedLineTerminators { matcher, searcher } => {
                write!(
                    f,
                    "grep config error: mismatched line terminators, \
                     matcher has {:?} but searcher has {:?}",
                    matcher, searcher
                )
            }
        }
    }
}

/// The internal configuration of a searcher.
#[derive(Clone, Debug)]
pub(crate) struct Config {
    /// The line terminator to use.
    pub(crate) line_term: LineTerminator,
    /// Whether to count line numbers.
    pub(crate) line_number: bool,
    /// Whether to enable matching across multiple lines.
    multi_line: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            line_term: LineTerminator::default(),
            line_number: true,
            multi_line: false,
        }
    }
}

/// A builder for configuring a searcher.
#[derive(Clone, Debug)]
pub struct SearcherBuilder {
    config: Config,
}

impl Default for SearcherBuilder {
    fn default() -> SearcherBuilder {
        SearcherBuilder::new()
    }
}

impl SearcherBuilder {
    /// Create a new searcher builder with a default configuration.
    pub fn new() -> SearcherBuilder {
        SearcherBuilder {
            config: Config::default(),
        }
    }

    /// Build a searcher.
    pub fn build(&self) -> Searcher {
        Searcher {
            config: self.config.clone(),
        }
    }

    /// Whether to count and include line numbers with matching lines.
    pub fn line_number(&mut self, yes: bool) -> &mut SearcherBuilder {
        self.config.line_number = yes;
        self
    }

    /// Whether to enable multi line search or not.
    pub fn multi_line(&mut self, yes: bool) -> &mut SearcherBuilder {
        self.config.multi_line = yes;
        self
    }
}

/// A searcher executes searches over a haystack and writes results to a caller
/// provided sink.
#[derive(Clone, Debug)]
pub struct Searcher {
    pub(crate) config: Config,
}

impl Searcher {
    /// Create a new searcher with a default configuration.
    pub fn new() -> Searcher {
        SearcherBuilder::new().build()
    }

    /// Execute a search over the given slice and write the results to the
    /// given sink.
    pub fn search_slice<M, S>(
        &mut self,
        matcher: M,
        slice: &[u8],
        write_to: S,
    ) -> Result<(), S::Error>
    where
        M: Matcher,
        S: Sink,
    {
        self.check_config(&matcher)
            .map_err(S::Error::error_message)?;

        if self.multi_line_with_matcher(&matcher) {
            MultiLine::new(self, matcher, slice, write_to).run()
        } else {
            SliceByLine::new(self, matcher, slice, write_to).run()
        }
    }

    /// Check that the searcher's configuration and the matcher are consistent.
    fn check_config<M: Matcher>(&self, matcher: M) -> Result<(), ConfigError> {
        let matcher_line_term = match matcher.line_terminator() {
            None => return Ok(()),
            Some(line_term) => line_term,
        };
        if matcher_line_term != self.config.line_term {
            return Err(ConfigError::MismatchedLineTerminators {
                matcher: matcher_line_term,
                searcher: self.config.line_term,
            });
        }
        Ok(())
    }
}

/// Configuration query methods used by the sink and internal search core.
impl Searcher {
    /// Returns the line terminator used by this searcher.
    #[inline]
    pub fn line_terminator(&self) -> LineTerminator {
        self.config.line_term
    }

    /// Returns true if and only if this searcher is configured to count line
    /// numbers.
    #[inline]
    pub fn line_number(&self) -> bool {
        self.config.line_number
    }

    /// Returns true if and only if this searcher is configured to perform
    /// multi line search.
    #[inline]
    pub fn multi_line(&self) -> bool {
        self.config.multi_line
    }

    /// Returns true if and only if this searcher will choose a multi-line
    /// strategy given the provided matcher.
    pub fn multi_line_with_matcher<M: Matcher>(&self, matcher: M) -> bool {
        if !self.multi_line() {
            return false;
        }
        if let Some(line_term) = matcher.line_terminator() {
            if line_term == self.line_terminator() {
                return false;
            }
        }
        if let Some(non_matching) = matcher.non_matching_bytes() {
            if non_matching.contains(self.line_terminator().as_byte()) {
                return false;
            }
        }
        true
    }
}
