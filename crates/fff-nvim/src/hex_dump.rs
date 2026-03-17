use mlua::prelude::*;
use std::fmt::Write as _;
use std::io::{Read, Seek, SeekFrom};

// Byte category colors (matching hexyl's default theme)
const COLOR_OFFSET: &str = "#888888";
const COLOR_NULL: &str = "#555753";
const COLOR_ASCII_PRINTABLE: &str = "#06989a";
const COLOR_ASCII_WHITESPACE: &str = "#4e9a06";
const COLOR_ASCII_OTHER: &str = "#4e9a06";
const COLOR_NON_ASCII: &str = "#c4a000";

fn byte_color(b: u8) -> &'static str {
    match b {
        0x00 => COLOR_NULL,
        0x20 | 0x09 | 0x0a | 0x0d => COLOR_ASCII_WHITESPACE,
        0x21..=0x7e => COLOR_ASCII_PRINTABLE,
        0x01..=0x1f | 0x7f => COLOR_ASCII_OTHER,
        _ => COLOR_NON_ASCII,
    }
}

fn byte_char(b: u8) -> char {
    match b {
        0x20..=0x7e => b as char,
        _ => '.',
    }
}

const BYTES_PER_LINE: usize = 16;

struct Span {
    line: usize,
    col_start: usize,
    col_end: usize,
    color: &'static str,
}

/// Push a span, merging with the previous one if same line and color.
fn push_span(
    spans: &mut Vec<Span>,
    line: usize,
    col_start: usize,
    col_end: usize,
    color: &'static str,
) {
    if let Some(last) = spans.last_mut() {
        // Merge if same line, same color, and adjacent (allow small gaps for spaces between hex pairs)
        if last.line == line && std::ptr::eq(last.color, color) && col_start <= last.col_end + 1 {
            last.col_end = col_end;
            return;
        }
    }
    spans.push(Span {
        line,
        col_start,
        col_end,
        color,
    });
}

/// Format raw bytes into hex dump lines with coalesced highlight spans.
///
/// Layout per line:
/// ```text
/// XXXXXXXX  HH HH HH HH HH HH HH HH  HH HH HH HH HH HH HH HH  CCCCCCCCCCCCCCCC
/// ```
fn format_hex_dump(raw_bytes: &[u8], base_offset: u64) -> (Vec<String>, Vec<Span>) {
    let mut lines = Vec::new();
    let mut spans = Vec::new();

    for (chunk_idx, chunk) in raw_bytes.chunks(BYTES_PER_LINE).enumerate() {
        let addr = base_offset + (chunk_idx * BYTES_PER_LINE) as u64;
        let mut line = format!("{addr:08x}  ");

        // Offset label highlight
        push_span(&mut spans, chunk_idx, 0, 8, COLOR_OFFSET);

        // Hex pairs with a gap after 8 bytes
        for (i, &b) in chunk.iter().enumerate() {
            if i == 8 {
                line.push(' ');
            }
            let col = line.len();
            push_span(&mut spans, chunk_idx, col, col + 2, byte_color(b));
            write!(line, "{b:02x} ").unwrap();
        }

        // Pad if the last line is short
        if chunk.len() < BYTES_PER_LINE {
            let missing = BYTES_PER_LINE - chunk.len();
            let mut pad = missing * 3;
            if chunk.len() <= 8 {
                pad += 1;
            }
            for _ in 0..pad {
                line.push(' ');
            }
        }

        // Separator before char panel
        line.push(' ');

        // Character panel — consecutive same-color chars merge automatically
        let char_start = line.len();
        for (i, &b) in chunk.iter().enumerate() {
            let col = char_start + i;
            push_span(&mut spans, chunk_idx, col, col + 1, byte_color(b));
            line.push(byte_char(b));
        }

        lines.push(line);
    }

    (lines, spans)
}

/// Generate a hex dump for a binary file with paging support and highlight data.
///
/// Returns a Lua table:
/// ```text
/// {
///   lines: string[],
///   highlights: {line_0idx, col_start, col_end, color}[],
///   has_more: bool,
///   next_offset: number,
/// }
/// ```
pub fn hex_dump(
    lua: &Lua,
    (file_path, offset, length): (String, Option<u64>, Option<u64>),
) -> LuaResult<LuaValue> {
    let offset = offset.unwrap_or(0);
    let length = length.unwrap_or(4096);

    let file = std::fs::File::open(&file_path)
        .map_err(|e| LuaError::RuntimeError(format!("Failed to open file: {e}")))?;

    let file_size = file
        .metadata()
        .map_err(|e| LuaError::RuntimeError(format!("Failed to get metadata: {e}")))?
        .len();

    let table = lua.create_table()?;

    if offset >= file_size {
        table.set("lines", lua.create_table()?)?;
        table.set("highlights", lua.create_table()?)?;
        table.set("has_more", false)?;
        table.set("next_offset", file_size)?;
        return Ok(LuaValue::Table(table));
    }

    let mut reader = std::io::BufReader::new(file);
    reader
        .seek(SeekFrom::Start(offset))
        .map_err(|e| LuaError::RuntimeError(format!("Failed to seek: {e}")))?;
    let mut raw_bytes = Vec::with_capacity(length as usize);
    reader
        .by_ref()
        .take(length)
        .read_to_end(&mut raw_bytes)
        .map_err(|e| LuaError::RuntimeError(format!("Failed to read: {e}")))?;

    let (plain_lines, hl_spans) = format_hex_dump(&raw_bytes, offset);

    let lines_table = lua.create_table()?;
    for (i, line) in plain_lines.iter().enumerate() {
        lines_table.set(i + 1, line.as_str())?;
    }
    table.set("lines", lines_table)?;

    let highlights_table = lua.create_table()?;
    for (i, span) in hl_spans.iter().enumerate() {
        let hl = lua.create_table()?;
        hl.raw_set(1, span.line)?;
        hl.raw_set(2, span.col_start)?;
        hl.raw_set(3, span.col_end)?;
        hl.raw_set(4, span.color)?;
        highlights_table.raw_set(i + 1, hl)?;
    }
    table.set("highlights", highlights_table)?;

    let bytes_read = raw_bytes.len() as u64;
    let next_offset = offset + bytes_read;
    table.set("has_more", next_offset < file_size)?;
    table.set("next_offset", next_offset)?;

    Ok(LuaValue::Table(table))
}
