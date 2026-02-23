use crate::ConstraintVec;
use crate::config::ParserConfig;
use crate::constraints::{Constraint, GitStatusFilter, TextPartsBuffer};
use crate::location::{Location, parse_location};
use zlob::{ZlobFlags, has_wildcards};

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum FuzzyQuery<'a> {
    Parts(TextPartsBuffer<'a>),
    Text(&'a str),
    Empty,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FFFQuery<'a> {
    /// Parsed constraints (stack-allocated for ≤8 constraints)
    pub constraints: ConstraintVec<'a>,
    pub fuzzy_query: FuzzyQuery<'a>,
    /// Parsed location (e.g., file:12:4 -> line 12, col 4)
    pub location: Option<Location>,
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

    pub fn parse<'a>(&self, query: &'a str) -> Option<FFFQuery<'a>> {
        let query: &'a str = query;
        let config: &C = &self.config;
        let mut constraints = ConstraintVec::new();
        let query = query.trim();

        let whitespace_count = query.chars().filter(|c| c.is_whitespace()).count();

        // Single token - check if it's a constraint or plain text
        if whitespace_count == 0 {
            // Try to parse as constraint first
            if let Some(constraint) = parse_token(query, config) {
                constraints.push(constraint);
                return Some(FFFQuery {
                    constraints,
                    fuzzy_query: FuzzyQuery::Empty,
                    location: None,
                });
            }

            // Try to extract location from single token (e.g., "file:12")
            let (query_without_loc, location) = parse_location(query);
            if location.is_some() {
                return Some(FFFQuery {
                    constraints,
                    fuzzy_query: FuzzyQuery::Text(query_without_loc),
                    location,
                });
            }

            // Plain text single token - return None (caller handles as simple fuzzy match)
            return None;
        }

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

        // Try to extract location from the last fuzzy token
        // e.g., "search file:12" -> fuzzy="search file", location=Line(12)
        let location = if !text_parts.is_empty() {
            let last_idx = text_parts.len() - 1;
            let (without_loc, loc) = parse_location(text_parts[last_idx]);
            if loc.is_some() {
                // Update the last part to be without the location suffix
                text_parts[last_idx] = without_loc;
                loc
            } else {
                None
            }
        } else {
            None
        };

        let fuzzy_query = if text_parts.is_empty() {
            FuzzyQuery::Empty
        } else if text_parts.len() == 1 {
            // If the only remaining text is empty after location extraction, treat as Empty
            if text_parts[0].is_empty() {
                FuzzyQuery::Empty
            } else {
                FuzzyQuery::Text(text_parts[0])
            }
        } else {
            // Filter out empty parts that might result from location extraction
            if text_parts.iter().all(|p| p.is_empty()) {
                FuzzyQuery::Empty
            } else {
                FuzzyQuery::Parts(text_parts)
            }
        };

        Some(FFFQuery {
            constraints,
            fuzzy_query,
            location,
        })
    }
}

impl<'a> FFFQuery<'a> {
    /// Returns the grep search text by joining all non-constraint text tokens.
    ///
    /// Backslash-escaped tokens (e.g. `\*.rs`) are included as literal text
    /// with the leading `\` stripped, since the backslash is only an escape
    /// signal to the parser and should not appear in the final pattern.
    ///
    /// `FuzzyQuery::Empty` → empty string  
    /// `FuzzyQuery::Text("foo")` → `"foo"`  
    /// `FuzzyQuery::Parts(["a", "\\*.rs", "b"])` → `"a *.rs b"`
    pub fn grep_text(&self) -> String {
        match &self.fuzzy_query {
            FuzzyQuery::Empty => String::new(),
            FuzzyQuery::Text(t) => strip_leading_backslash(t).to_string(),
            FuzzyQuery::Parts(parts) => parts
                .iter()
                .map(|t| strip_leading_backslash(t))
                .collect::<Vec<_>>()
                .join(" "),
        }
    }
}

/// Strip the leading `\` from a backslash-escaped token, returning the rest.
/// For all other tokens returns the input unchanged.
#[inline]
fn strip_leading_backslash(token: &str) -> &str {
    if token.starts_with('\\') && token.len() > 1 {
        &token[1..]
    } else {
        token
    }
}

impl Default for QueryParser<crate::FilePickerConfig> {
    fn default() -> Self {
        Self::new(crate::FilePickerConfig)
    }
}

#[inline]
fn parse_token<'a, C: ParserConfig>(token: &'a str, config: &C) -> Option<Constraint<'a>> {
    // Backslash escape: \token → treat as literal text, skip all constraint parsing.
    // The leading \ is stripped by the caller when building the search text.
    if token.starts_with('\\') && token.len() > 1 {
        return None;
    }

    let first_byte = token.as_bytes().first()?;

    match first_byte {
        b'*' if config.enable_extension() => {
            // Ignore incomplete patterns like "*" or "*."
            if token == "*" || token == "*." {
                return None;
            }

            // Try extension first (*.rs) - simple patterns without additional wildcards
            if let Some(constraint) = parse_extension(token) {
                // Only return Extension if the rest doesn't have wildcards
                // e.g., *.rs is Extension, but *.test.* should be Glob
                let ext_part = &token[2..];
                if !has_wildcards(ext_part, ZlobFlags::RECOMMENDED) {
                    return Some(constraint);
                }
            }
            // Has wildcards -> use config-specific glob detection
            if config.enable_glob() && config.is_glob_pattern(token) {
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
            // Check for glob patterns using config-specific detection
            if config.enable_glob() && config.is_glob_pattern(token) {
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
                    "status" | "st" | "g" | "git" if config.enable_git_status() => {
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
    // Backslash escape applies here too
    if token.starts_with('\\') && token.len() > 1 {
        return None;
    }

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
            // Has wildcards -> use config-specific glob detection
            if config.enable_glob() && config.is_glob_pattern(token) {
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
            // Check for glob patterns using config-specific detection
            if config.enable_glob() && config.is_glob_pattern(token) {
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
                    "status" | "gi" | "g" | "st" if config.enable_git_status() => {
                        return parse_git_status(value);
                    }
                    _ => {}
                }
            }

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
    use crate::{FilePickerConfig, GrepConfig};

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
    fn test_incomplete_patterns_ignored() {
        let config = FilePickerConfig;
        // Incomplete patterns should return None and be treated as noise
        assert_eq!(parse_token("*", &config), None);
        assert_eq!(parse_token("*.", &config), None);
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
        let result = parser
            .parse("www/ test")
            .expect("Should parse multi-token query");
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
        // Need two tokens for parsing to return Some
        let result = parser
            .parse("!test foo")
            .expect("Should parse multi-token query");
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
        let result = parser
            .parse("!*.rs foo")
            .expect("Should parse multi-token query");
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
        let result = parser
            .parse("!/src/ foo")
            .expect("Should parse multi-token query");
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
        let result = parser
            .parse("!status:modified foo")
            .expect("Should parse multi-token query");
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

    #[test]
    fn test_backslash_escape_extension() {
        let parser = QueryParser::new(FilePickerConfig);
        let result = parser
            .parse("\\*.rs foo")
            .expect("Should parse multi-token query");
        // \*.rs should NOT be parsed as an Extension constraint
        assert_eq!(result.constraints.len(), 0);
        // Both tokens should be text
        match result.fuzzy_query {
            FuzzyQuery::Parts(parts) => {
                assert_eq!(parts.len(), 2);
                assert_eq!(parts[0], "\\*.rs");
                assert_eq!(parts[1], "foo");
            }
            _ => panic!("Expected Parts, got {:?}", result.fuzzy_query),
        }
    }

    #[test]
    fn test_backslash_escape_path_segment() {
        let parser = QueryParser::new(FilePickerConfig);
        let result = parser
            .parse("\\/src/ foo")
            .expect("Should parse multi-token query");
        assert_eq!(result.constraints.len(), 0);
        match result.fuzzy_query {
            FuzzyQuery::Parts(parts) => {
                assert_eq!(parts[0], "\\/src/");
                assert_eq!(parts[1], "foo");
            }
            _ => panic!("Expected Parts, got {:?}", result.fuzzy_query),
        }
    }

    #[test]
    fn test_backslash_escape_negation() {
        let parser = QueryParser::new(FilePickerConfig);
        let result = parser
            .parse("\\!test foo")
            .expect("Should parse multi-token query");
        assert_eq!(result.constraints.len(), 0);
    }

    #[test]
    fn test_grep_text_plain_text() {
        // Multi-token plain text — no constraints
        let q = QueryParser::new(GrepConfig)
            .parse("name =")
            .expect("should parse");
        assert_eq!(q.grep_text(), "name =");
    }

    #[test]
    fn test_grep_text_strips_constraint() {
        let q = QueryParser::new(GrepConfig)
            .parse("name = *.rs someth")
            .expect("should parse");
        assert_eq!(q.grep_text(), "name = someth");
    }

    #[test]
    fn test_grep_text_leading_constraint() {
        let q = QueryParser::new(GrepConfig)
            .parse("*.rs name =")
            .expect("should parse");
        assert_eq!(q.grep_text(), "name =");
    }

    #[test]
    fn test_grep_text_only_constraints() {
        let q = QueryParser::new(GrepConfig)
            .parse("*.rs /src/")
            .expect("should parse");
        assert_eq!(q.grep_text(), "");
    }

    #[test]
    fn test_grep_text_path_constraint() {
        let q = QueryParser::new(GrepConfig)
            .parse("name /src/ value")
            .expect("should parse");
        assert_eq!(q.grep_text(), "name value");
    }

    #[test]
    fn test_grep_text_negation_constraint() {
        let q = QueryParser::new(GrepConfig)
            .parse("name !*.rs value")
            .expect("should parse");
        assert_eq!(q.grep_text(), "name value");
    }

    #[test]
    fn test_grep_text_backslash_escape_stripped() {
        // \*.rs should be text with the leading \ removed
        let q = QueryParser::new(GrepConfig)
            .parse("\\*.rs foo")
            .expect("should parse");
        assert_eq!(q.grep_text(), "*.rs foo");

        let q = QueryParser::new(GrepConfig)
            .parse("\\/src/ foo")
            .expect("should parse");
        assert_eq!(q.grep_text(), "/src/ foo");

        let q = QueryParser::new(GrepConfig)
            .parse("\\!test foo")
            .expect("should parse");
        assert_eq!(q.grep_text(), "!test foo");
    }

    #[test]
    fn test_grep_text_question_mark_is_text() {
        let q = QueryParser::new(GrepConfig)
            .parse("foo? bar")
            .expect("should parse");
        assert_eq!(q.grep_text(), "foo? bar");
    }

    #[test]
    fn test_grep_text_bracket_is_text() {
        let q = QueryParser::new(GrepConfig)
            .parse("arr[0] more")
            .expect("should parse");
        assert_eq!(q.grep_text(), "arr[0] more");
    }

    #[test]
    fn test_grep_text_path_glob_is_constraint() {
        let q = QueryParser::new(GrepConfig)
            .parse("pattern src/**/*.rs")
            .expect("should parse");
        assert_eq!(q.grep_text(), "pattern");
    }

    #[test]
    fn test_grep_question_mark_is_text() {
        let parser = QueryParser::new(GrepConfig);
        // Single token "foo?" should return None (treated as plain text by caller)
        let result = parser.parse("foo?");
        assert!(result.is_none(), "foo? should be plain text in grep mode");
    }

    #[test]
    fn test_grep_bracket_is_text() {
        let parser = QueryParser::new(GrepConfig);
        let result = parser.parse("arr[0] something");
        let result = result.expect("Should parse multi-token query");
        // arr[0] should NOT be a glob in grep mode
        assert_eq!(result.constraints.len(), 0);
    }

    #[test]
    fn test_grep_path_glob_is_constraint() {
        let parser = QueryParser::new(GrepConfig);
        let result = parser
            .parse("pattern src/**/*.rs")
            .expect("Should parse with path glob");
        // src/**/*.rs contains / so it should be treated as a glob
        assert_eq!(result.constraints.len(), 1);
        assert!(matches!(
            result.constraints[0],
            Constraint::Glob("src/**/*.rs")
        ));
    }

    #[test]
    fn test_grep_brace_is_constraint() {
        let parser = QueryParser::new(GrepConfig);
        let result = parser
            .parse("pattern {src,lib}")
            .expect("Should parse with brace expansion");
        assert_eq!(result.constraints.len(), 1);
        assert!(matches!(
            result.constraints[0],
            Constraint::Glob("{src,lib}")
        ));
    }

    #[test]
    fn test_grep_bare_star_is_text() {
        let parser = QueryParser::new(GrepConfig);
        // "a*b" contains * but no / or {} — should be text in grep mode
        let result = parser.parse("a*b something");
        let result = result.expect("Should parse");
        assert_eq!(
            result.constraints.len(),
            0,
            "bare * without / should be text"
        );
    }

    #[test]
    fn test_grep_negated_text() {
        let parser = QueryParser::new(GrepConfig);
        let result = parser
            .parse("pattern !test")
            .expect("Should parse negated text in grep mode");
        assert_eq!(result.constraints.len(), 1);
        match &result.constraints[0] {
            Constraint::Not(inner) => {
                assert!(
                    matches!(**inner, Constraint::Text("test")),
                    "Expected Not(Text(\"test\")), got Not({:?})",
                    inner
                );
            }
            other => panic!("Expected Not constraint, got {:?}", other),
        }
    }

    #[test]
    fn test_grep_negated_path_segment() {
        let parser = QueryParser::new(GrepConfig);
        let result = parser
            .parse("pattern !/src/")
            .expect("Should parse negated path segment in grep mode");
        assert_eq!(result.constraints.len(), 1);
        match &result.constraints[0] {
            Constraint::Not(inner) => {
                assert!(
                    matches!(**inner, Constraint::PathSegment("src")),
                    "Expected Not(PathSegment(\"src\")), got Not({:?})",
                    inner
                );
            }
            other => panic!("Expected Not constraint, got {:?}", other),
        }
    }

    #[test]
    fn test_grep_negated_extension() {
        let parser = QueryParser::new(GrepConfig);
        let result = parser
            .parse("pattern !*.rs")
            .expect("Should parse negated extension in grep mode");
        assert_eq!(result.constraints.len(), 1);
        match &result.constraints[0] {
            Constraint::Not(inner) => {
                assert!(
                    matches!(**inner, Constraint::Extension("rs")),
                    "Expected Not(Extension(\"rs\")), got Not({:?})",
                    inner
                );
            }
            other => panic!("Expected Not constraint, got {:?}", other),
        }
    }
}
