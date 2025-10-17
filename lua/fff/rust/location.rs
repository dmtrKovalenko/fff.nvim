#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub enum Location {
    Line(i32),
    Range { start: (i32, i32), end: (i32, i32) },
    Position { line: i32, col: i32 },
}

fn parse_number_pair(location: &str, split_char: char) -> Option<(i32, i32)> {
    let mut iter = location.split(split_char);

    let start_str = iter.next()?;
    let end_str = iter.next()?;

    // if there are more than 2 parts it's not the range treat as normal query
    if iter.next().is_some() {
        return None;
    }

    let start = start_str.parse::<i32>().ok()?;
    let end = end_str.parse::<i32>().ok()?;

    Some((start, end))
}

/// Parse "line-line" format
fn parse_simple_range(location: &str) -> Option<Location> {
    let (start, end) = parse_number_pair(location, '-')?;
    if end < start {
        return Some(Location::Line(start));
    }

    Some(Location::Range {
        start: (start, 0),
        end: (end, 0),
    })
}

/// Parse "line:col-col" format (column range on same line)
fn parse_column_range(start_part: &str, end_part: &str) -> Option<Location> {
    let (line_str, start_col_str) = start_part.split_once(':')?;
    let line = line_str.parse::<i32>().ok()?;
    let start_col = start_col_str.parse::<i32>().ok()?;
    let end_col = end_part.parse::<i32>().ok()?;

    if end_col < start_col {
        return Some(Location::Line(line));
    }

    Some(Location::Range {
        start: (line, start_col),
        end: (line, end_col),
    })
}

/// Parse "line:col-line:col" format (position range)
fn parse_position_range(start_part: &str, end_part: &str) -> Option<Location> {
    let (start_line, start_col) = parse_number_pair(start_part, ':')?;
    let (end_line, end_col) = parse_number_pair(end_part, ':')?;

    if end_line < start_line || (end_line == start_line && end_col < start_col) {
        return Some(Location::Position {
            line: start_line,
            col: start_col,
        });
    }

    Some(Location::Range {
        start: (start_line, start_col),
        end: (end_line, end_col),
    })
}

/// Try to parse range patterns (contains '-')
fn try_parse_column_range(location: &str) -> Option<Location> {
    if !location.contains('-') {
        return None;
    }

    let (start_part, end_part) = location.split_once('-')?;

    // Try position range (line:col-line:col)
    if start_part.contains(':') && end_part.contains(':') {
        return parse_position_range(start_part, end_part);
    }

    // Try column range (line:col-col)
    if start_part.contains(':') {
        return parse_column_range(start_part, end_part);
    }

    // Try simple line range (line-line)
    parse_simple_range(location)
}

/// Try to parse position patterns (contains ':' but not '-')
fn try_parse_column_position(location: &str) -> Option<Location> {
    if !location.contains(':') {
        return None;
    }

    let (line_str, col_str) = location.split_once(':')?;
    let line = line_str.parse::<i32>().ok()?;
    let col = col_str.parse::<i32>().ok()?;

    Some(Location::Position { line, col })
}

/// Parses various location formats like file:12, file:12:4, file:12-114
fn parse_column_location(query: &str) -> Option<(&str, Location)> {
    let (file_path, location_part) = query.split_once(':')?;

    if let Some(range_location) = try_parse_column_range(location_part) {
        return Some((file_path, range_location));
    }

    if let Some(position_location) = try_parse_column_position(location_part) {
        return Some((file_path, position_location));
    }

    if let Ok(line_location) = location_part.parse::<i32>() {
        return Some((file_path, Location::Line(line_location)));
    }

    None
}

fn parse_vstudio_location(query: &str) -> Option<(&str, Location)> {
    if !query.ends_with(')') {
        return None;
    }

    let (file_path, location_with_paren) = query.rsplit_once('(')?;
    let location = location_with_paren.trim_end_matches(')');

    if let Ok(line) = location.parse::<i32>() {
        return Some((file_path, Location::Line(line)));
    }

    if let Some((line, col)) = parse_number_pair(location, ',') {
        return Some((file_path, Location::Position { line, col }));
    }

    None
}

pub fn parse_location(query: &str) -> (&str, Option<Location>) {
    // simply ignore the last semicolon even if there are no additional location info
    let query = query.trim_end_matches([':', '-', '(']);
    if let Some((path, location)) = parse_column_location(query) {
        return (path, Some(location));
    }

    if let Some((path, location)) = parse_vstudio_location(query) {
        return (path, Some(location));
    }

    (query, None)
}

#[cfg(test)]
mod tests {
    pub use super::*;

    #[test]
    fn test_location_parsing() {
        assert_eq!(
            parse_location("new_file:12"),
            ("new_file", Some(Location::Line(12)))
        );
        assert_eq!(parse_location("new_file:12ab"), ("new_file:12ab", None));

        assert_eq!(parse_location("something"), ("something", None));
        assert_eq!(
            parse_location("file:12:4"),
            ("file", Some(Location::Position { line: 12, col: 4 }))
        );

        assert_eq!(
            parse_location("file:12-114"),
            (
                "file",
                Some(Location::Range {
                    start: (12, 0),
                    end: (114, 0)
                })
            )
        );

        assert_eq!(
            parse_location("file:12:4-20"),
            (
                "file",
                Some(Location::Range {
                    start: (12, 4),
                    end: (12, 20)
                })
            )
        );

        assert_eq!(
            parse_location("file:100:4-14:20"),
            ("file", Some(Location::Position { line: 100, col: 4 }))
        );

        assert_eq!(
            parse_location("file:12:4-14:20"),
            (
                "file",
                Some(Location::Range {
                    start: (12, 4),
                    end: (14, 20)
                })
            )
        );
    }

    #[test]
    fn test_vstudio_parsing() {
        assert_eq!(
            parse_location("file(12)"),
            ("file", Some(Location::Line(12)))
        );
        assert_eq!(
            parse_location("file(12,4)"),
            ("file", Some(Location::Position { line: 12, col: 4 }))
        );
    }

    #[test]
    fn trimes_end_character() {
        assert_eq!(
            parse_location("file:12-"),
            ("file", Some(Location::Line(12)))
        );
        assert_eq!(parse_location("file:-"), ("file", None));
        assert_eq!(parse_location("file("), ("file", None));
    }
}
