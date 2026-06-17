//! Formats search results into `rg`-compatible terminal output.
//!
//! Takes structured [`GrepResult`]/[`SearchResult`] data from the search
//! engine and renders it to an arbitrary [`Write`] sink (typically the
//! client's stdout fd received over the Unix socket). Output style is
//! controlled by [`OutputFlags`].

use std::collections::HashSet;
use std::io::{BufWriter, Write};

use fff::{FilePicker, GrepMatch, GrepResult, SearchResult};
use fff_ipc_domain::OutputFlags;

// ANSI escape sequences matching rg's default color scheme.
const MAGENTA: &str = "\x1b[35m";
const GREEN: &str = "\x1b[32m";
const RED_BOLD: &str = "\x1b[1m\x1b[31m";
const RESET: &str = "\x1b[0m";

/// Writes search results to a buffered sink, formatting according to [`OutputFlags`].
pub struct ResultWriter<W: Write> {
    out: BufWriter<W>,
    cfg: OutputFlags,
}

impl<W: Write> ResultWriter<W> {
    pub fn new(out: W, cfg: OutputFlags) -> Self {
        Self { out: BufWriter::new(out), cfg }
    }

    /// Dispatches grep results to the appropriate output mode.
    /// Returns `Ok(true)` if any matches were written.
    pub fn write_grep(
        &mut self,
        picker: &FilePicker,
        result: &GrepResult<'_>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        if self.cfg.contains(OutputFlags::QUIET) {
            return Ok(!result.matches.is_empty());
        }
        if self.cfg.contains(OutputFlags::FILES_ONLY) {
            return self.write_files_with_matches(picker, result);
        }
        if self.cfg.contains(OutputFlags::COUNT_ONLY) {
            return self.write_counts(picker, result);
        }
        if self.cfg.contains(OutputFlags::VIMGREP) {
            return self.write_vimgrep(picker, result);
        }

        if self.cfg.contains(OutputFlags::HEADING) {
            self.write_heading_mode(picker, result)?;
        } else {
            self.write_inline_mode(picker, result)?;
        }
        self.out.flush()?;
        Ok(!result.matches.is_empty())
    }

    /// Matches grouped under a filename header, separated by blank lines.
    fn write_heading_mode(
        &mut self,
        picker: &FilePicker,
        result: &GrepResult<'_>,
    ) -> std::io::Result<()> {
        let mut current_file: Option<usize> = None;
        let mut prev_line: Option<u64> = None;

        for m in &result.matches {
            if current_file != Some(m.file_index) {
                if current_file.is_some() {
                    writeln!(self.out)?;
                }
                let path = result.files[m.file_index].relative_path(picker);
                self.write_path(&path)?;
                writeln!(self.out)?;
                current_file = Some(m.file_index);
                prev_line = None;
            }

            self.write_context_separator(m, &mut prev_line)?;
            self.write_context_before(m, None, &mut prev_line)?;
            self.write_match_line(m, None)?;
            self.write_context_after(m, None, &mut prev_line)?;
            prev_line = Some(m.line_number + m.context_after.len() as u64);
        }
        Ok(())
    }

    /// Each match line prefixed with `path:` when `WITH_FILENAME` is set.
    fn write_inline_mode(
        &mut self,
        picker: &FilePicker,
        result: &GrepResult<'_>,
    ) -> std::io::Result<()> {
        let mut current_file: Option<usize> = None;
        let mut prev_line: Option<u64> = None;

        for m in &result.matches {
            if current_file != Some(m.file_index) {
                current_file = Some(m.file_index);
                prev_line = None;
            }

            let path = if self.cfg.contains(OutputFlags::WITH_FILENAME) {
                Some(result.files[m.file_index].relative_path(picker))
            } else {
                None
            };

            self.write_context_separator(m, &mut prev_line)?;
            self.write_context_before(m, path.as_deref(), &mut prev_line)?;
            self.write_match_line(m, path.as_deref())?;
            self.write_context_after(m, path.as_deref(), &mut prev_line)?;
            prev_line = Some(m.line_number + m.context_after.len() as u64);
        }
        Ok(())
    }

    /// Writes a file path, magenta when color is on.
    fn write_path(&mut self, path: &str) -> std::io::Result<()> {
        if self.cfg.contains(OutputFlags::COLOR) {
            write!(self.out, "{RESET}{MAGENTA}{path}{RESET}")
        } else {
            write!(self.out, "{path}")
        }
    }

    /// Writes `N:` or `N-` (for context lines). No-op when `LINE_NUMBER` is off.
    fn write_line_number(&mut self, n: u64, sep: char) -> std::io::Result<()> {
        if self.cfg.contains(OutputFlags::LINE_NUMBER) {
            if self.cfg.contains(OutputFlags::COLOR) {
                write!(self.out, "{RESET}{GREEN}{n}{RESET}{sep}")
            } else {
                write!(self.out, "{n}{sep}")
            }
        } else {
            Ok(())
        }
    }

    /// Renders one match: optional path prefix, line number, column, and content.
    fn write_match_line(
        &mut self,
        m: &GrepMatch,
        inline_path: Option<&str>,
    ) -> std::io::Result<()> {
        if let Some(path) = inline_path {
            self.write_path(path)?;
            write!(self.out, ":")?;
        }
        self.write_line_number(m.line_number, ':')?;
        if self.cfg.contains(OutputFlags::COLUMN) {
            let col = m.col + 1;
            if self.cfg.contains(OutputFlags::COLOR) {
                write!(self.out, "{RESET}{col}{RESET}:")?;
            } else {
                write!(self.out, "{col}:")?;
            }
        }
        if self.cfg.contains(OutputFlags::COLOR) && !m.match_byte_offsets.is_empty() {
            self.write_highlighted(&m.line_content, &m.match_byte_offsets)
        } else {
            writeln!(self.out, "{}", m.line_content)
        }
    }

    /// Writes a line with match spans wrapped in bold red ANSI codes.
    fn write_highlighted(&mut self, line: &str, offsets: &[(u32, u32)]) -> std::io::Result<()> {
        let bytes = line.as_bytes();
        let mut pos = 0usize;
        for &(start, end) in offsets {
            let s = start as usize;
            let e = (end as usize).min(bytes.len());
            if s > pos {
                self.out.write_all(&bytes[pos..s.min(bytes.len())])?;
            }
            if s < e {
                write!(self.out, "{RESET}{RED_BOLD}")?;
                self.out.write_all(&bytes[s..e])?;
                write!(self.out, "{RESET}")?;
            }
            pos = e;
        }
        if pos < bytes.len() {
            self.out.write_all(&bytes[pos..])?;
        }
        writeln!(self.out)
    }

    /// Prints `--` between non-contiguous context blocks.
    fn write_context_separator(
        &mut self,
        m: &GrepMatch,
        prev_line: &mut Option<u64>,
    ) -> std::io::Result<()> {
        if let Some(prev) = *prev_line {
            let has_context = !m.context_before.is_empty() || !m.context_after.is_empty();
            if has_context {
                let context_start = m.line_number.saturating_sub(m.context_before.len() as u64);
                if context_start > prev + 1 {
                    writeln!(self.out, "--")?;
                }
            }
        }
        Ok(())
    }

    /// Writes context lines before a match, skipping any that overlap with previous output.
    fn write_context_before(
        &mut self,
        m: &GrepMatch,
        inline_path: Option<&str>,
        prev_line: &mut Option<u64>,
    ) -> std::io::Result<()> {
        let start_line = m.line_number.saturating_sub(m.context_before.len() as u64);
        for (i, line) in m.context_before.iter().enumerate() {
            let line_num = start_line + i as u64;
            if let Some(prev) = *prev_line
                && line_num <= prev
            {
                continue;
            }
            if let Some(path) = inline_path {
                self.write_path(path)?;
                write!(self.out, "-")?;
            }
            self.write_line_number(line_num, '-')?;
            writeln!(self.out, "{line}")?;
        }
        Ok(())
    }

    /// Writes context lines after a match.
    fn write_context_after(
        &mut self,
        m: &GrepMatch,
        inline_path: Option<&str>,
        prev_line: &mut Option<u64>,
    ) -> std::io::Result<()> {
        for (i, line) in m.context_after.iter().enumerate() {
            let line_num = m.line_number + 1 + i as u64;
            if let Some(path) = inline_path {
                self.write_path(path)?;
                write!(self.out, "-")?;
            }
            self.write_line_number(line_num, '-')?;
            writeln!(self.out, "{line}")?;
        }
        *prev_line = Some(m.line_number + m.context_after.len() as u64);
        Ok(())
    }

    /// `file:line:col:text` format for editor integration.
    fn write_vimgrep(
        &mut self,
        picker: &FilePicker,
        result: &GrepResult<'_>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        for m in &result.matches {
            let path = result.files[m.file_index].relative_path(picker);
            let col = m.col + 1;
            writeln!(self.out, "{path}:{}:{col}:{}", m.line_number, m.line_content)?;
        }
        self.out.flush()?;
        Ok(!result.matches.is_empty())
    }

    /// Prints per-file match counts.
    fn write_counts(
        &mut self,
        picker: &FilePicker,
        result: &GrepResult<'_>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let mut file_counts: Vec<(usize, usize)> = Vec::new();
        for m in &result.matches {
            match file_counts.last_mut() {
                Some((idx, count)) if *idx == m.file_index => *count += 1,
                _ => file_counts.push((m.file_index, 1)),
            }
        }
        for (file_idx, count) in &file_counts {
            let path = result.files[*file_idx].relative_path(picker);
            if self.cfg.contains(OutputFlags::WITH_FILENAME) {
                if self.cfg.contains(OutputFlags::COLOR) {
                    writeln!(self.out, "{RESET}{MAGENTA}{path}{RESET}:{count}")?;
                } else {
                    writeln!(self.out, "{path}:{count}")?;
                }
            } else {
                writeln!(self.out, "{count}")?;
            }
        }
        self.out.flush()?;
        Ok(!file_counts.is_empty())
    }

    /// Prints only the names of files that contain matches.
    fn write_files_with_matches(
        &mut self,
        picker: &FilePicker,
        result: &GrepResult<'_>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let mut seen = HashSet::new();
        for m in &result.matches {
            if seen.insert(m.file_index) {
                let path = result.files[m.file_index].relative_path(picker);
                self.write_path(&path)?;
                writeln!(self.out)?;
            }
        }
        self.out.flush()?;
        Ok(!seen.is_empty())
    }

    /// Writes fuzzy file-search results, one path per line.
    pub fn write_files(
        &mut self,
        picker: &FilePicker,
        result: &SearchResult<'_>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        if self.cfg.contains(OutputFlags::QUIET) {
            return Ok(!result.items.is_empty());
        }
        for item in &result.items {
            writeln!(self.out, "{}", item.relative_path(picker))?;
        }
        self.out.flush()?;
        Ok(!result.items.is_empty())
    }
}
