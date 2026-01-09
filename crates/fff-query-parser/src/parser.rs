use crate::ConstraintVec;
use crate::config::ParserConfig;
use crate::constraints::{Constraint, GitStatusFilter, TextPartsBuffer};
use zlob::{ZlobFlags, has_wildcards};

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum FuzzyQuery<'a> {
    Parts(TextPartsBuffer<'a>),
    Text(&'a str),
    Empty,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParseResult<'a> {
    /// Parsed constraints (stack-allocated for ≤8 constraints)
    pub constraints: ConstraintVec<'a>,
    pub fuzzy_query: FuzzyQuery<'a>,
}

/// Main query parser - zero-cost wrapper around configuration
#[derive(Debug)]
pub struct QueryParser<C: ParserConfig> {
    config: C,
}

impl<C: ParserConfig> QueryParser<C> {
    pub fn new(config: C) -> Self {
        Self { config }
    }

    pub fn parse<'a>(&self, query: &'a str) -> Option<ParseResult<'a>> {
        let query: &'a str = query;
        let config: &C = &self.config;
        let mut constraints = ConstraintVec::new();
        let query = query.trim();

        let whitespace_count = query.chars().filter(|c| c.is_whitespace()).count();
        if whitespace_count == 0 {
            return None;
        }

        // Stack-allocated buffer for text parts (up to 16 parts)
        let mut text_parts = TextPartsBuffer::new();
        let tokens = query.split_whitespace();

        for token in tokens {
            match parse_token(token, config) {
                Some(constraint) => {
                    constraints.push(constraint);
                }
                None => {
                    text_parts.push(token);
                }
            }
        }

        let fuzzy_query = if text_parts.is_empty() {
            FuzzyQuery::Empty
        } else if text_parts.len() == 1 {
            FuzzyQuery::Text(text_parts[0])
        } else {
            FuzzyQuery::Parts(text_parts)
        };

        Some(ParseResult {
            constraints,
            fuzzy_query,
        })
    }
}

impl Default for QueryParser<crate::FilePickerConfig> {
    fn default() -> Self {
        Self::new(crate::FilePickerConfig)
    }
}

#[inline]
fn parse_token<'a, C: ParserConfig>(token: &'a str, config: &C) -> Option<Constraint<'a>> {
    let first_byte = token.as_bytes().first()?;

    match first_byte {
        b'*' if config.enable_extension() => {
            // Try extension first (*.rs) - simple patterns without additional wildcards
            if let Some(constraint) = parse_extension(token) {
                // Only return Extension if the rest doesn't have wildcards
                // e.g., *.rs is Extension, but *.test.* should be Glob
                let ext_part = &token[2..];
                if !has_wildcards(ext_part, ZlobFlags::RECOMMENDED) {
                    return Some(constraint);
                }
            }
            // Has wildcards -> use zlob for matching
            if config.enable_glob() && has_wildcards(token, ZlobFlags::RECOMMENDED) {
                return Some(Constraint::Glob(token));
            }
            None
        }
        b'!' if config.enable_exclude() => parse_negation(token, config),
        b'/' if config.enable_path_segments() => parse_path_segment(token),
        _ if config.enable_path_segments() && token.ends_with('/') => {
            // Handle trailing slash syntax: www/ -> PathSegment("www")
            parse_path_segment_trailing(token)
        }
        _ => {
            // Check for glob patterns using zlob's SIMD-optimized detection
            if config.enable_glob() && has_wildcards(token, ZlobFlags::RECOMMENDED) {
                return Some(Constraint::Glob(token));
            }

            // Check for key:value patterns
            if let Some(colon_idx) = memchr(b':', token.as_bytes()) {
                let (key, value_with_colon) = token.split_at(colon_idx);
                let value = &value_with_colon[1..]; // Skip the colon

                match key {
                    "type" if config.enable_type_filter() => {
                        return Some(Constraint::FileType(value));
                    }
                    "status" if config.enable_git_status() => {
                        return parse_git_status(value);
                    }
                    _ => {}
                }
            }

            // Try custom parsers
            config.parse_custom(token)
        }
    }
}

/// Find first occurrence of byte in slice (fast memchr-like implementation)
#[inline]
fn memchr(needle: u8, haystack: &[u8]) -> Option<usize> {
    haystack.iter().position(|&b| b == needle)
}

/// Parse extension pattern: *.rs -> Extension("rs")
#[inline]
fn parse_extension(token: &str) -> Option<Constraint<'_>> {
    if token.len() > 2 && token.starts_with("*.") {
        Some(Constraint::Extension(&token[2..]))
    } else {
        None
    }
}

/// Parse negation pattern: !*.rs -> Not(Extension("rs")), !test -> Not(Text("test"))
/// This allows negating any constraint type
#[inline]
fn parse_negation<'a, C: ParserConfig>(token: &'a str, config: &C) -> Option<Constraint<'a>> {
    if token.len() <= 1 {
        return None;
    }

    let inner_token = &token[1..];

    // Try to parse the inner token as any constraint
    if let Some(inner_constraint) = parse_token_without_negation(inner_token, config) {
        // Wrap it in a Not constraint
        return Some(Constraint::Not(Box::new(inner_constraint)));
    }

    // If it's not a special constraint, treat it as negated text
    // For backward compatibility with !test syntax
    Some(Constraint::Not(Box::new(Constraint::Text(inner_token))))
}

/// Parse a token without checking for negation (to avoid infinite recursion)
#[inline]
fn parse_token_without_negation<'a, C: ParserConfig>(
    token: &'a str,
    config: &C,
) -> Option<Constraint<'a>> {
    let first_byte = token.as_bytes().first()?;

    match first_byte {
        b'*' if config.enable_extension() => {
            // Try extension first (*.rs) - simple patterns without additional wildcards
            if let Some(constraint) = parse_extension(token) {
                let ext_part = &token[2..];
                if !has_wildcards(ext_part, ZlobFlags::RECOMMENDED) {
                    return Some(constraint);
                }
            }
            // Has wildcards -> use zlob for matching
            if config.enable_glob() && has_wildcards(token, ZlobFlags::RECOMMENDED) {
                return Some(Constraint::Glob(token));
            }
            None
        }
        b'/' if config.enable_path_segments() => parse_path_segment(token),
        _ if config.enable_path_segments() && token.ends_with('/') => {
            // Handle trailing slash syntax: www/ -> PathSegment("www")
            parse_path_segment_trailing(token)
        }
        _ => {
            // Check for glob patterns using zlob's SIMD-optimized detection
            if config.enable_glob() && has_wildcards(token, ZlobFlags::RECOMMENDED) {
                return Some(Constraint::Glob(token));
            }

            // Check for key:value patterns
            if let Some(colon_idx) = memchr(b':', token.as_bytes()) {
                let (key, value_with_colon) = token.split_at(colon_idx);
                let value = &value_with_colon[1..]; // Skip the colon

                match key {
                    "type" if config.enable_type_filter() => {
                        return Some(Constraint::FileType(value));
                    }
                    "status" if config.enable_git_status() => {
                        return parse_git_status(value);
                    }
                    _ => {}
                }
            }

            // Try custom parsers
            config.parse_custom(token)
        }
    }
}

/// Parse path segment: /src/ -> PathSegment("src")
#[inline]
fn parse_path_segment(token: &str) -> Option<Constraint<'_>> {
    if token.len() > 1 && token.starts_with('/') {
        let segment = token.trim_start_matches('/').trim_end_matches('/');
        if !segment.is_empty() {
            Some(Constraint::PathSegment(segment))
        } else {
            None
        }
    } else {
        None
    }
}

/// Parse path segment with trailing slash: www/ -> PathSegment("www")
#[inline]
fn parse_path_segment_trailing(token: &str) -> Option<Constraint<'_>> {
    if token.len() > 1 && token.ends_with('/') {
        let segment = token.trim_end_matches('/');
        if !segment.is_empty() && !segment.contains('/') {
            Some(Constraint::PathSegment(segment))
        } else {
            None
        }
    } else {
        None
    }
}

/// Parse git status filter: modified|m|untracked|u|staged|s
#[inline]
fn parse_git_status(value: &str) -> Option<Constraint<'_>> {
    if value == "*" {
        return None;
    }

    if "modified".starts_with(value) {
        return Some(Constraint::GitStatus(GitStatusFilter::Modified));
    }

    if "untracked".starts_with(value) {
        return Some(Constraint::GitStatus(GitStatusFilter::Untracked));
    }

    if "staged".starts_with(value) {
        return Some(Constraint::GitStatus(GitStatusFilter::Staged));
    }

    if "clean".starts_with(value) {
        return Some(Constraint::GitStatus(GitStatusFilter::Unmodified));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FilePickerConfig;

    #[test]
    fn test_parse_extension() {
        assert_eq!(parse_extension("*.rs"), Some(Constraint::Extension("rs")));
        assert_eq!(
            parse_extension("*.toml"),
            Some(Constraint::Extension("toml"))
        );
        assert_eq!(parse_extension("*"), None);
        assert_eq!(parse_extension("*."), None);
    }

    #[test]
    fn test_parse_path_segment() {
        assert_eq!(
            parse_path_segment("/src/"),
            Some(Constraint::PathSegment("src"))
        );
        assert_eq!(
            parse_path_segment("/lib"),
            Some(Constraint::PathSegment("lib"))
        );
        assert_eq!(parse_path_segment("/"), None);
    }

    #[test]
    fn test_parse_path_segment_trailing() {
        assert_eq!(
            parse_path_segment_trailing("www/"),
            Some(Constraint::PathSegment("www"))
        );
        assert_eq!(
            parse_path_segment_trailing("src/"),
            Some(Constraint::PathSegment("src"))
        );
        // Should not match paths with multiple segments
        assert_eq!(parse_path_segment_trailing("src/lib/"), None);
        // Should not match without trailing slash
        assert_eq!(parse_path_segment_trailing("www"), None);
    }

    #[test]
    fn test_trailing_slash_in_query() {
        let parser = QueryParser::new(FilePickerConfig);
        let result = parser.parse("www/ test");
        assert_eq!(result.constraints.len(), 1);
        assert!(matches!(
            result.constraints[0],
            Constraint::PathSegment("www")
        ));
        assert!(matches!(result.fuzzy_query, FuzzyQuery::Text("test")));
    }

    #[test]
    fn test_parse_git_status() {
        assert_eq!(
            parse_git_status("modified"),
            Some(Constraint::GitStatus(GitStatusFilter::Modified))
        );
        assert_eq!(
            parse_git_status("m"),
            Some(Constraint::GitStatus(GitStatusFilter::Modified))
        );
        assert_eq!(
            parse_git_status("untracked"),
            Some(Constraint::GitStatus(GitStatusFilter::Untracked))
        );
        assert_eq!(parse_git_status("invalid"), None);
    }

    #[test]
    fn test_memchr() {
        assert_eq!(memchr(b':', b"type:rust"), Some(4));
        assert_eq!(memchr(b':', b"nocolon"), None);
        assert_eq!(memchr(b':', b":start"), Some(0));
    }

    #[test]
    fn test_negation_text() {
        let parser = QueryParser::new(FilePickerConfig);
        let result = parser.parse("!test");
        assert_eq!(result.constraints.len(), 1);
        match &result.constraints[0] {
            Constraint::Not(inner) => {
                assert!(matches!(**inner, Constraint::Text("test")));
            }
            _ => panic!("Expected Not constraint"),
        }
    }

    #[test]
    fn test_negation_extension() {
        let parser = QueryParser::new(FilePickerConfig);
        let result = parser.parse("!*.rs");
        assert_eq!(result.constraints.len(), 1);
        match &result.constraints[0] {
            Constraint::Not(inner) => {
                assert!(matches!(**inner, Constraint::Extension("rs")));
            }
            _ => panic!("Expected Not(Extension) constraint"),
        }
    }

    #[test]
    fn test_negation_path_segment() {
        let parser = QueryParser::new(FilePickerConfig);
        let result = parser.parse("!/src/");
        assert_eq!(result.constraints.len(), 1);
        match &result.constraints[0] {
            Constraint::Not(inner) => {
                assert!(matches!(**inner, Constraint::PathSegment("src")));
            }
            _ => panic!("Expected Not(PathSegment) constraint"),
        }
    }

    #[test]
    fn test_negation_git_status() {
        let parser = QueryParser::new(FilePickerConfig);
        let result = parser.parse("!status:modified");
        assert_eq!(result.constraints.len(), 1);
        match &result.constraints[0] {
            Constraint::Not(inner) => {
                assert!(matches!(
                    **inner,
                    Constraint::GitStatus(GitStatusFilter::Modified)
                ));
            }
            _ => panic!("Expected Not(GitStatus) constraint"),
        }
    }
}
