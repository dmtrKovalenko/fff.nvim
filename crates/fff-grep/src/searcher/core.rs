use grep_matcher::{LineMatchKind, Matcher};

use crate::{
    lines::{self, LineStep},
    searcher::{Config, Range, Searcher},
    sink::{Sink, SinkError, SinkFinish, SinkMatch},
};

#[derive(Debug)]
pub(crate) struct Core<'s, M: 's, S> {
    config: &'s Config,
    matcher: M,
    searcher: &'s Searcher,
    sink: S,
    pos: usize,
    absolute_byte_offset: u64,
    line_number: Option<u64>,
    last_line_counted: usize,
    last_line_visited: usize,
}

impl<'s, M: Matcher, S: Sink> Core<'s, M, S> {
    pub(crate) fn new(searcher: &'s Searcher, matcher: M, sink: S) -> Core<'s, M, S> {
        let line_number = if searcher.config.line_number {
            Some(1)
        } else {
            None
        };
        Core {
            config: &searcher.config,
            matcher,
            searcher,
            sink,
            pos: 0,
            absolute_byte_offset: 0,
            line_number,
            last_line_counted: 0,
            last_line_visited: 0,
        }
    }

    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    pub(crate) fn set_pos(&mut self, pos: usize) {
        self.pos = pos;
    }

    pub(crate) fn matched(&mut self, buf: &[u8], range: &Range) -> Result<bool, S::Error> {
        self.sink_matched(buf, range)
    }

    pub(crate) fn find(&mut self, slice: &[u8]) -> Result<Option<Range>, S::Error> {
        match self.matcher.find(slice) {
            Err(err) => Err(S::Error::error_message(err)),
            Ok(m) => Ok(m),
        }
    }

    fn shortest_match(&mut self, slice: &[u8]) -> Result<Option<usize>, S::Error> {
        match self.matcher.shortest_match(slice) {
            Err(err) => Err(S::Error::error_message(err)),
            Ok(m) => Ok(m),
        }
    }

    pub(crate) fn begin(&mut self) -> Result<bool, S::Error> {
        self.sink.begin(self.searcher)
    }

    pub(crate) fn finish(&mut self, byte_count: u64) -> Result<(), S::Error> {
        self.sink.finish(self.searcher, &SinkFinish { byte_count })
    }

    pub(crate) fn match_by_line(&mut self, buf: &[u8]) -> Result<bool, S::Error> {
        if self.is_line_by_line_fast() {
            self.match_by_line_fast(buf)
        } else {
            self.match_by_line_slow(buf)
        }
    }

    fn match_by_line_slow(&mut self, buf: &[u8]) -> Result<bool, S::Error> {
        debug_assert!(!self.searcher.multi_line_with_matcher(&self.matcher));

        let range = Range::new(self.pos(), buf.len());
        let mut stepper =
            LineStep::new(self.config.line_term.as_byte(), range.start(), range.end());
        while let Some(line) = stepper.next_match(buf) {
            let matched = {
                let slice = lines::without_terminator(&buf[line], self.config.line_term);
                self.shortest_match(slice)?.is_some()
            };
            self.set_pos(line.end());
            if matched && !self.sink_matched(buf, &line)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn match_by_line_fast(&mut self, buf: &[u8]) -> Result<bool, S::Error> {
        while !buf[self.pos()..].is_empty() {
            if let Some(line) = self.find_by_line_fast(buf)? {
                self.set_pos(line.end());
                if !self.sink_matched(buf, &line)? {
                    return Ok(false);
                }
            } else {
                break;
            }
        }
        self.set_pos(buf.len());
        Ok(true)
    }

    #[inline(always)]
    fn find_by_line_fast(&mut self, buf: &[u8]) -> Result<Option<Range>, S::Error> {
        debug_assert!(!self.searcher.multi_line_with_matcher(&self.matcher));
        debug_assert!(self.is_line_by_line_fast());

        let mut pos = self.pos();
        while !buf[pos..].is_empty() {
            match self.matcher.find_candidate_line(&buf[pos..]) {
                Err(err) => return Err(S::Error::error_message(err)),
                Ok(None) => return Ok(None),
                Ok(Some(LineMatchKind::Confirmed(i))) => {
                    let line = lines::locate(
                        buf,
                        self.config.line_term.as_byte(),
                        Range::zero(i).offset(pos),
                    );
                    if line.start() == buf.len() {
                        pos = buf.len();
                        continue;
                    }
                    return Ok(Some(line));
                }
                Ok(Some(LineMatchKind::Candidate(i))) => {
                    let line = lines::locate(
                        buf,
                        self.config.line_term.as_byte(),
                        Range::zero(i).offset(pos),
                    );
                    let slice = lines::without_terminator(&buf[line], self.config.line_term);
                    if self
                        .matcher
                        .is_match(slice)
                        .map_err(S::Error::error_message)?
                    {
                        return Ok(Some(line));
                    }
                    pos = line.end();
                }
            }
        }
        Ok(None)
    }

    #[inline(always)]
    fn sink_matched(&mut self, buf: &[u8], range: &Range) -> Result<bool, S::Error> {
        self.count_lines(buf, range.start());
        let offset = self.absolute_byte_offset + range.start() as u64;
        let linebuf = &buf[*range];
        let keepgoing = self.sink.matched(
            self.searcher,
            &SinkMatch {
                bytes: linebuf,
                absolute_byte_offset: offset,
                line_number: self.line_number,
                buffer: buf,
                bytes_range_in_buffer: range.start()..range.end(),
            },
        )?;
        if !keepgoing {
            return Ok(false);
        }
        self.last_line_visited = range.end();
        Ok(true)
    }

    fn count_lines(&mut self, buf: &[u8], upto: usize) {
        if let Some(ref mut line_number) = self.line_number {
            if self.last_line_counted >= upto {
                return;
            }
            let slice = &buf[self.last_line_counted..upto];
            let count = lines::count(slice, self.config.line_term.as_byte());
            *line_number += count;
            self.last_line_counted = upto;
        }
    }

    fn is_line_by_line_fast(&self) -> bool {
        debug_assert!(!self.searcher.multi_line_with_matcher(&self.matcher));
        if let Some(line_term) = self.matcher.line_terminator() {
            if line_term.as_byte() == b'\x00' {
                return false;
            }
            if line_term == self.config.line_term {
                return true;
            }
        }
        if let Some(non_matching) = self.matcher.non_matching_bytes()
            && non_matching.contains(self.config.line_term.as_byte())
        {
            return true;
        }
        false
    }
}
