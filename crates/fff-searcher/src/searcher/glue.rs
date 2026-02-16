use grep_matcher::Matcher;

use crate::{
    lines,
    searcher::{Config, Range, Searcher, core::Core},
    sink::Sink,
};

#[derive(Debug)]
pub(crate) struct SliceByLine<'s, M, S> {
    core: Core<'s, M, S>,
    slice: &'s [u8],
}

impl<'s, M: Matcher, S: Sink> SliceByLine<'s, M, S> {
    pub(crate) fn new(
        searcher: &'s Searcher,
        matcher: M,
        slice: &'s [u8],
        write_to: S,
    ) -> SliceByLine<'s, M, S> {
        debug_assert!(!searcher.multi_line_with_matcher(&matcher));

        SliceByLine {
            core: Core::new(searcher, matcher, write_to),
            slice,
        }
    }

    pub(crate) fn run(mut self) -> Result<(), S::Error> {
        if self.core.begin()? {
            while !self.slice[self.core.pos()..].is_empty()
                && self.core.match_by_line(self.slice)?
            {}
        }
        let byte_count = self.slice.len() as u64;
        self.core.finish(byte_count)
    }
}

#[derive(Debug)]
pub(crate) struct MultiLine<'s, M, S> {
    config: &'s Config,
    core: Core<'s, M, S>,
    slice: &'s [u8],
    last_match: Option<Range>,
}

impl<'s, M: Matcher, S: Sink> MultiLine<'s, M, S> {
    pub(crate) fn new(
        searcher: &'s Searcher,
        matcher: M,
        slice: &'s [u8],
        write_to: S,
    ) -> MultiLine<'s, M, S> {
        debug_assert!(searcher.multi_line_with_matcher(&matcher));

        MultiLine {
            config: &searcher.config,
            core: Core::new(searcher, matcher, write_to),
            slice,
            last_match: None,
        }
    }

    pub(crate) fn run(mut self) -> Result<(), S::Error> {
        if self.core.begin()? {
            let mut keepgoing = true;
            while !self.slice[self.core.pos()..].is_empty() && keepgoing {
                keepgoing = self.sink()?;
            }
            if keepgoing && let Some(last_match) = self.last_match.take() {
                self.sink_matched(&last_match)?;
            }
        }
        let byte_count = self.slice.len() as u64;
        self.core.finish(byte_count)
    }

    fn sink(&mut self) -> Result<bool, S::Error> {
        let mat = match self.find()? {
            Some(range) => range,
            None => {
                self.core.set_pos(self.slice.len());
                return Ok(true);
            }
        };
        self.advance(&mat);

        let line = lines::locate(self.slice, self.config.line_term.as_byte(), mat);
        match self.last_match.take() {
            None => {
                self.last_match = Some(line);
                Ok(true)
            }
            Some(last_match) => {
                if last_match.end() >= line.start() {
                    self.last_match = Some(last_match.with_end(line.end()));
                    Ok(true)
                } else {
                    self.last_match = Some(line);
                    self.sink_matched(&last_match)
                }
            }
        }
    }

    fn sink_matched(&mut self, range: &Range) -> Result<bool, S::Error> {
        if range.is_empty() {
            return Ok(false);
        }
        self.core.matched(self.slice, range)
    }

    fn find(&mut self) -> Result<Option<Range>, S::Error> {
        self.core
            .find(&self.slice[self.core.pos()..])
            .map(|m| m.map(|m| m.offset(self.core.pos())))
    }

    fn advance(&mut self, range: &Range) {
        self.core.set_pos(range.end());
        if range.is_empty() && self.core.pos() < self.slice.len() {
            let newpos = self.core.pos() + 1;
            self.core.set_pos(newpos);
        }
    }
}
